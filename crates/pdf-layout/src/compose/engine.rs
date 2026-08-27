use super::*;

pub(super) enum FontSlot {
    Standard(StdFont),
    Embedded(EmbeddedSlot),
}

pub(super) struct EmbeddedSlot {
    pub(super) program: Vec<u8>,
    pub(super) info: pdf_fonts::FontInfo,
    pub(super) used: RefCell<BTreeMap<u16, (u16, char)>>,
}

impl EmbeddedSlot {
    pub(super) fn cid_font(&self) -> CidFont {
        let used = self.used.borrow();
        let mut widths: Vec<_> = used
            .iter()
            .map(|(gid, (width, _))| (*gid, *width))
            .collect();
        widths.sort_unstable();
        let to_unicode = used.iter().map(|(gid, (_, ch))| (*gid, *ch)).collect();
        let used_gids: Vec<_> = used.keys().copied().collect();
        let (program, cid_to_gid) = match pdf_fonts::subset_with_map(&self.program, &used_gids) {
            Some((program, map)) => (program, Some(cid_to_gid_map(&map))),
            None => (self.program.clone(), None),
        };
        CidFont {
            program,
            postscript_name: self.info.postscript_name.clone(),
            ascent: self.info.ascent,
            descent: self.info.descent,
            cap_height: self.info.cap_height,
            bbox: self.info.bbox,
            italic_angle: self.info.italic_angle,
            flags: if self.info.italic { 4 | 64 } else { 4 },
            default_width: 1000,
            widths,
            to_unicode,
            cid_to_gid,
        }
    }
}

pub(super) fn cid_to_gid_map(map: &[(u16, u16)]) -> Vec<u8> {
    let max_cid = map.iter().map(|(cid, _)| *cid).max().unwrap_or(0) as usize;
    let mut bytes = vec![0u8; (max_cid + 1) * 2];
    for &(cid, gid) in map {
        let offset = cid as usize * 2;
        bytes[offset..offset + 2].copy_from_slice(&gid.to_be_bytes());
    }
    bytes
}

pub(super) struct Metrics<'a> {
    fonts: &'a BTreeMap<String, FontSlot>,
}

impl<'a> Metrics<'a> {
    pub(super) fn new(fonts: &'a BTreeMap<String, FontSlot>) -> Self {
        Self { fonts }
    }

    fn for_resource(&self, resource: &str) -> Result<Box<dyn FontMetrics + '_>, ComposeError> {
        match self.fonts.get(resource) {
            Some(FontSlot::Standard(font)) => Ok(Box::new(StandardMetrics::new(font.base_name()))),
            Some(FontSlot::Embedded(slot)) => {
                Ok(Box::new(EmbeddedMetrics::new(&slot.program, &slot.info)))
            }
            None => Err(ComposeError::MissingFont(resource.to_string())),
        }
    }

    fn glyphs(&self, resource: &str, text: &str) -> Result<Option<Vec<u16>>, ComposeError> {
        let Some(slot) = self.fonts.get(resource) else {
            return Err(ComposeError::MissingFont(resource.to_string()));
        };
        let FontSlot::Embedded(slot) = slot else {
            return Ok(None);
        };
        let glyphs = pdf_fonts::shape_text(&slot.program, text).ok_or(ComposeError::InvalidFont)?;
        let mut used = slot.used.borrow_mut();
        for glyph in &glyphs {
            used.entry(glyph.id).or_insert((glyph.advance, glyph.ch));
        }
        Ok(Some(glyphs.into_iter().map(|glyph| glyph.id).collect()))
    }
}

pub(super) trait Element {
    fn measure(&mut self, available: Size, metrics: &Metrics) -> Result<Plan, ComposeError>;
    fn draw(&mut self, context: &mut DrawCtx<'_>, space: Size) -> Result<(), ComposeError>;
    fn reset(&mut self);
}

pub(super) enum Node {
    Column(ColumnNode),
    Decorated(DecoratedNode),
    Image(ImageNode),
    PageBreak(PageBreakNode),
    Row(RowNode),
    Semantic(SemanticNode),
    Table(TableNode),
    Text(TextNode),
}

impl Node {
    pub(super) fn empty_column() -> Self {
        Self::Column(ColumnNode::new(Vec::new(), 0.0))
    }

    pub(super) fn has_remaining(&self) -> bool {
        match self {
            Node::Column(column) => column.index < column.children.len(),
            Node::Decorated(decorated) => decorated.child.has_remaining(),
            Node::Image(image) => !image.complete,
            Node::PageBreak(page_break) => !page_break.complete,
            Node::Row(row) => !row.complete,
            Node::Semantic(semantic) => semantic.child.has_remaining(),
            Node::Table(table) => table.row_index < table.rows.len(),
            Node::Text(text) => !text.complete,
        }
    }

    pub(super) fn set_page_numbers(&mut self, page: usize, pages: usize) {
        match self {
            Node::Column(column) => {
                for child in &mut column.children {
                    child.set_page_numbers(page, pages);
                }
            }
            Node::Decorated(decorated) => decorated.child.set_page_numbers(page, pages),
            Node::Image(_) => {}
            Node::PageBreak(_) => {}
            Node::Row(row) => {
                for (_, child) in &mut row.children {
                    child.set_page_numbers(page, pages);
                }
            }
            Node::Semantic(semantic) => semantic.child.set_page_numbers(page, pages),
            Node::Table(table) => {
                if let Some(header) = &mut table.header {
                    for (_, child) in &mut header.children {
                        child.set_page_numbers(page, pages);
                    }
                }
                for row in &mut table.rows {
                    for (_, child) in &mut row.children {
                        child.set_page_numbers(page, pages);
                    }
                }
            }
            Node::Text(text) => {
                text.page = page;
                text.pages = pages;
            }
        }
    }

    pub(super) fn collect_structure(
        &self,
        output: &mut Vec<StructElem>,
    ) -> Result<(), ComposeError> {
        match self {
            Node::Column(column) => {
                for child in &column.children {
                    child.collect_structure(output)?;
                }
            }
            Node::Decorated(decorated) => decorated.child.collect_structure(output)?,
            Node::Image(_) | Node::PageBreak(_) | Node::Text(_) => {}
            Node::Row(row) => {
                for (_, child) in &row.children {
                    child.collect_structure(output)?;
                }
            }
            Node::Semantic(semantic) => output.push(semantic.structure()?),
            Node::Table(table) => {
                if let Some(header) = &table.header {
                    let mut row = StructElem::new("TR");
                    let mut cells = Vec::new();
                    for (_, child) in &header.children {
                        child.collect_structure(&mut cells)?;
                    }
                    for cell in cells {
                        row.push_child(cell);
                    }
                    output.push(row);
                }
                for table_row in &table.rows {
                    let mut row = StructElem::new("TR");
                    let mut cells = Vec::new();
                    for (_, child) in &table_row.children {
                        child.collect_structure(&mut cells)?;
                    }
                    for cell in cells {
                        row.push_child(cell);
                    }
                    output.push(row);
                }
            }
        }
        Ok(())
    }
}

impl Element for Node {
    fn measure(&mut self, available: Size, metrics: &Metrics) -> Result<Plan, ComposeError> {
        match self {
            Node::Column(column) => column.measure(available, metrics),
            Node::Decorated(decorated) => decorated.measure(available, metrics),
            Node::Image(image) => image.measure(available, metrics),
            Node::PageBreak(page_break) => page_break.measure(available, metrics),
            Node::Row(row) => row.measure(available, metrics),
            Node::Semantic(semantic) => semantic.measure(available, metrics),
            Node::Table(table) => table.measure(available, metrics),
            Node::Text(text) => text.measure(available, metrics),
        }
    }

    fn draw(&mut self, context: &mut DrawCtx<'_>, space: Size) -> Result<(), ComposeError> {
        match self {
            Node::Column(column) => column.draw(context, space),
            Node::Decorated(decorated) => decorated.draw(context, space),
            Node::Image(image) => image.draw(context, space),
            Node::PageBreak(page_break) => page_break.draw(context, space),
            Node::Row(row) => row.draw(context, space),
            Node::Semantic(semantic) => semantic.draw(context, space),
            Node::Table(table) => table.draw(context, space),
            Node::Text(text) => text.draw(context, space),
        }
    }

    fn reset(&mut self) {
        match self {
            Node::Column(column) => column.reset(),
            Node::Decorated(decorated) => decorated.reset(),
            Node::Image(image) => image.reset(),
            Node::PageBreak(page_break) => page_break.reset(),
            Node::Row(row) => row.reset(),
            Node::Semantic(semantic) => semantic.reset(),
            Node::Table(table) => table.reset(),
            Node::Text(text) => text.reset(),
        }
    }
}

pub(super) struct PlannedChild {
    index: usize,
    size: Size,
}

pub(super) struct ColumnNode {
    children: Vec<Node>,
    spacing: f64,
    index: usize,
    planned: Vec<PlannedChild>,
    measured: Option<(Plan, Size)>,
}

impl ColumnNode {
    pub(super) fn new(children: Vec<Node>, spacing: f64) -> Self {
        Self {
            children,
            spacing,
            index: 0,
            planned: Vec::new(),
            measured: None,
        }
    }
}

impl Element for ColumnNode {
    fn measure(&mut self, available: Size, metrics: &Metrics) -> Result<Plan, ComposeError> {
        if !available.is_valid() || !self.spacing.is_finite() || self.spacing < 0.0 {
            return Err(ComposeError::InvalidGeometry);
        }
        self.planned.clear();
        let mut used = 0.0;
        let mut any = false;
        let mut partial = false;

        for index in self.index..self.children.len() {
            let spacing = if any { self.spacing } else { 0.0 };
            if matches!(self.children[index], Node::PageBreak(_)) {
                if any {
                    self.planned.push(PlannedChild {
                        index,
                        size: Size::default(),
                    });
                    partial = true;
                    break;
                }
                if let Node::PageBreak(page_break) = &mut self.children[index] {
                    page_break.complete = true;
                }
                self.index = index + 1;
                continue;
            }
            let remaining = (available.height - used - spacing).max(0.0);
            let child_plan =
                self.children[index].measure(Size::new(available.width, remaining), metrics)?;
            match child_plan {
                Plan::Empty => {}
                Plan::Wrap => {
                    if !any {
                        self.measured = Some((Plan::Wrap, available));
                        return Ok(Plan::Wrap);
                    }
                    partial = true;
                    break;
                }
                Plan::Full(size) | Plan::Partial(size) => {
                    if !size.is_valid() || size.height > remaining + EPSILON {
                        return Err(ComposeError::MeasurementMismatch);
                    }
                    used += spacing + size.height;
                    any = true;
                    self.planned.push(PlannedChild { index, size });
                    if matches!(child_plan, Plan::Partial(_)) {
                        partial = true;
                        break;
                    }
                }
            }
        }

        let plan = if !any {
            Plan::Empty
        } else if partial
            || self
                .planned
                .last()
                .is_some_and(|p| p.index + 1 < self.children.len())
        {
            Plan::Partial(Size::new(available.width, used))
        } else {
            Plan::Full(Size::new(available.width, used))
        };
        self.measured = Some((plan, available));
        Ok(plan)
    }

    fn draw(&mut self, context: &mut DrawCtx<'_>, space: Size) -> Result<(), ComposeError> {
        let Some((plan, _available)) = self.measured else {
            return Err(ComposeError::MeasurementMismatch);
        };
        let expected = match plan {
            Plan::Full(size) | Plan::Partial(size) => size,
            Plan::Empty if space == Size::default() => return Ok(()),
            Plan::Empty | Plan::Wrap => return Err(ComposeError::MeasurementMismatch),
        };
        if expected != space {
            return Err(ComposeError::MeasurementMismatch);
        }

        let mut y = 0.0;
        for (position, child) in self.planned.iter().enumerate() {
            if position > 0 {
                y += self.spacing;
            }
            let child_index = child.index;
            let mut child_context = context.translated(0.0, y);
            self.children[child_index].draw(&mut child_context, child.size)?;
            y += child.size.height;
            if !self.children[child_index].has_remaining() {
                self.index = child_index + 1;
            } else {
                self.index = child_index;
                break;
            }
        }
        context.trace.events.push(GeometryEvent {
            page: context.page,
            kind: "Column",
            bounds: Rect {
                origin: context.origin,
                size: space,
            },
            text: None,
        });
        self.measured = None;
        Ok(())
    }

    fn reset(&mut self) {
        self.index = 0;
        self.planned.clear();
        self.measured = None;
        for child in &mut self.children {
            child.reset();
        }
    }
}

pub(super) struct PageBreakNode {
    pub(super) complete: bool,
}

pub(super) struct ImageNode {
    image: crate::Image,
    sizing: ImageSizing,
    measured: Option<Plan>,
    complete: bool,
}

impl ImageNode {
    pub(super) fn new(image: crate::Image, sizing: ImageSizing) -> Self {
        Self {
            image,
            sizing,
            measured: None,
            complete: false,
        }
    }

    fn box_size(&self) -> Size {
        match self.sizing {
            ImageSizing::Fit(size) | ImageSizing::Fill(size) | ImageSizing::Exact(size) => size,
        }
    }
}

impl Element for ImageNode {
    fn measure(&mut self, available: Size, _metrics: &Metrics) -> Result<Plan, ComposeError> {
        if self.complete {
            self.measured = Some(Plan::Empty);
            return Ok(Plan::Empty);
        }
        let requested = self.box_size();
        if !available.is_valid()
            || !requested.is_valid()
            || requested.width <= 0.0
            || requested.height <= 0.0
        {
            return Err(ComposeError::InvalidGeometry);
        }
        let intrinsic = Size::new(
            f64::from(self.image.width()),
            f64::from(self.image.height()),
        );
        let size = match self.sizing {
            ImageSizing::Fit(bounds) => {
                let scale = (bounds.width / intrinsic.width).min(bounds.height / intrinsic.height);
                Size::new(intrinsic.width * scale, intrinsic.height * scale)
            }
            ImageSizing::Fill(bounds) | ImageSizing::Exact(bounds) => bounds,
        };
        let plan =
            if size.width > available.width + EPSILON || size.height > available.height + EPSILON {
                Plan::Wrap
            } else {
                Plan::Full(size)
            };
        self.measured = Some(plan);
        Ok(plan)
    }

    fn draw(&mut self, context: &mut DrawCtx<'_>, space: Size) -> Result<(), ComposeError> {
        let Some(Plan::Full(expected)) = self.measured.take() else {
            return Err(ComposeError::MeasurementMismatch);
        };
        if expected != space {
            return Err(ComposeError::MeasurementMismatch);
        }
        let name = format!("Im{}", context.images.len());
        context
            .images
            .push((name.clone(), self.image.xobject.clone()));
        let intrinsic = Size::new(
            f64::from(self.image.width()),
            f64::from(self.image.height()),
        );
        let (draw_size, x, top) = match self.sizing {
            ImageSizing::Fit(_) | ImageSizing::Exact(_) => {
                (space, context.origin.x, context.origin.y)
            }
            ImageSizing::Fill(_) => {
                let scale = (space.width / intrinsic.width).max(space.height / intrinsic.height);
                let draw_size = Size::new(intrinsic.width * scale, intrinsic.height * scale);
                (
                    draw_size,
                    context.origin.x + (space.width - draw_size.width) / 2.0,
                    context.origin.y + (space.height - draw_size.height) / 2.0,
                )
            }
        };
        let pdf_y = context.page_height - top - draw_size.height;
        let artifact = context.tagged && *context.marked_depth == 0;
        if artifact {
            context.content.begin_artifact();
        }
        context.content.save();
        if matches!(self.sizing, ImageSizing::Fill(_)) {
            let clip_y = context.page_height - context.origin.y - space.height;
            context
                .content
                .rect(context.origin.x, clip_y, space.width, space.height)
                .clip()
                .end_path();
        }
        context
            .content
            .transform(draw_size.width, 0.0, 0.0, draw_size.height, x, pdf_y)
            .do_xobject(&name)
            .restore();
        if artifact {
            context.content.end_marked_content();
        }
        self.complete = true;
        context.trace.events.push(GeometryEvent {
            page: context.page,
            kind: "Image",
            bounds: Rect {
                origin: context.origin,
                size: space,
            },
            text: None,
        });
        Ok(())
    }

    fn reset(&mut self) {
        self.measured = None;
        self.complete = false;
    }
}

impl Element for PageBreakNode {
    fn measure(&mut self, _available: Size, _metrics: &Metrics) -> Result<Plan, ComposeError> {
        Ok(Plan::Empty)
    }

    fn draw(&mut self, _context: &mut DrawCtx<'_>, space: Size) -> Result<(), ComposeError> {
        if space != Size::default() {
            return Err(ComposeError::MeasurementMismatch);
        }
        self.complete = true;
        Ok(())
    }

    fn reset(&mut self) {
        self.complete = false;
    }
}

pub(super) struct PlannedRowChild {
    offset: f64,
    size: Option<Size>,
}

pub(super) struct RowNode {
    children: Vec<(RowWidth, Node)>,
    planned: Vec<PlannedRowChild>,
    measured: Option<Plan>,
    complete: bool,
}

impl RowNode {
    pub(super) fn new(children: Vec<(RowWidth, Node)>) -> Self {
        Self {
            children,
            planned: Vec::new(),
            measured: None,
            complete: false,
        }
    }

    fn widths(&mut self, available: Size, metrics: &Metrics) -> Result<Vec<f64>, ComposeError> {
        let mut fixed = 0.0;
        let mut relative = 0.0;
        let mut auto_widths = vec![0.0; self.children.len()];
        for (index, (width, child)) in self.children.iter_mut().enumerate() {
            match *width {
                RowWidth::Fixed(value) => {
                    if !value.is_finite() || value < 0.0 {
                        return Err(ComposeError::InvalidGeometry);
                    }
                    fixed += value;
                }
                RowWidth::Relative(factor) => {
                    if !factor.is_finite() || factor <= 0.0 {
                        return Err(ComposeError::InvalidGeometry);
                    }
                    relative += factor;
                }
                RowWidth::Auto => {
                    let plan = child.measure(available, metrics)?;
                    auto_widths[index] = match plan {
                        Plan::Full(size) | Plan::Partial(size) => size.width,
                        Plan::Empty | Plan::Wrap => 0.0,
                    };
                    child.reset();
                }
            }
        }
        let auto_total = auto_widths.iter().sum::<f64>();
        if fixed + auto_total > available.width + EPSILON {
            return Err(ComposeError::InvalidGeometry);
        }
        let relative_space = (available.width - fixed - auto_total).max(0.0);
        Ok(self
            .children
            .iter()
            .enumerate()
            .map(|(index, (width, _))| match *width {
                RowWidth::Fixed(value) => value,
                RowWidth::Relative(factor) => relative_space * factor / relative,
                RowWidth::Auto => auto_widths[index],
            })
            .collect())
    }

    fn measure_at_widths(
        &mut self,
        available: Size,
        widths: &[f64],
        metrics: &Metrics,
    ) -> Result<Plan, ComposeError> {
        self.planned.clear();
        let mut offset = 0.0;
        let mut height = 0.0f64;
        let mut any = false;
        for ((_, child), width) in self.children.iter_mut().zip(widths.iter().copied()) {
            let plan = child.measure(Size::new(width, available.height), metrics)?;
            let size = match plan {
                Plan::Empty => None,
                Plan::Full(size) => {
                    any = true;
                    height = height.max(size.height);
                    Some(size)
                }
                Plan::Partial(_) | Plan::Wrap => {
                    for (_, measured) in &mut self.children {
                        measured.reset();
                    }
                    self.planned.clear();
                    self.measured = Some(Plan::Wrap);
                    return Ok(Plan::Wrap);
                }
            };
            self.planned.push(PlannedRowChild { offset, size });
            offset += width;
        }
        let plan = if any {
            Plan::Full(Size::new(available.width, height))
        } else {
            Plan::Empty
        };
        self.measured = Some(plan);
        Ok(plan)
    }
}

impl Element for RowNode {
    fn measure(&mut self, available: Size, metrics: &Metrics) -> Result<Plan, ComposeError> {
        if self.complete {
            self.measured = Some(Plan::Empty);
            return Ok(Plan::Empty);
        }
        if !available.is_valid() {
            return Err(ComposeError::InvalidGeometry);
        }
        let widths = self.widths(available, metrics)?;
        self.measure_at_widths(available, &widths, metrics)
    }

    fn draw(&mut self, context: &mut DrawCtx<'_>, space: Size) -> Result<(), ComposeError> {
        let Some(plan) = self.measured else {
            return Err(ComposeError::MeasurementMismatch);
        };
        let Plan::Full(expected) = plan else {
            return Err(ComposeError::MeasurementMismatch);
        };
        if expected != space {
            return Err(ComposeError::MeasurementMismatch);
        }
        for (index, planned) in self.planned.iter().enumerate() {
            if let Some(size) = planned.size {
                let mut child_context = context.translated(planned.offset, 0.0);
                self.children[index].1.draw(&mut child_context, size)?;
            }
        }
        self.complete = true;
        self.measured = None;
        context.trace.events.push(GeometryEvent {
            page: context.page,
            kind: "Row",
            bounds: Rect {
                origin: context.origin,
                size: space,
            },
            text: None,
        });
        Ok(())
    }

    fn reset(&mut self) {
        self.planned.clear();
        self.measured = None;
        self.complete = false;
        for (_, child) in &mut self.children {
            child.reset();
        }
    }
}

pub(super) struct SemanticNode {
    semantic: Semantic,
    child: Box<Node>,
    marks: Vec<(usize, u32)>,
    annotation_indices: Vec<usize>,
}

impl SemanticNode {
    pub(super) fn new(semantic: Semantic, child: Node) -> Self {
        Self {
            semantic,
            child: Box::new(child),
            marks: Vec::new(),
            annotation_indices: Vec::new(),
        }
    }

    fn tag(&self) -> Result<String, ComposeError> {
        if let Semantic::Link { uri, description } = &self.semantic
            && (uri.is_empty() || description.is_empty())
        {
            return Err(ComposeError::InvalidGeometry);
        }
        Ok(match &self.semantic {
            Semantic::Paragraph => "P".to_string(),
            Semantic::Heading(level @ 1..=6) => format!("H{level}"),
            Semantic::Heading(_) => return Err(ComposeError::InvalidGeometry),
            Semantic::List => "L".to_string(),
            Semantic::ListItem => "LI".to_string(),
            Semantic::ListLabel => "Lbl".to_string(),
            Semantic::ListBody => "LBody".to_string(),
            Semantic::Table => "Table".to_string(),
            Semantic::TableRow => "TR".to_string(),
            Semantic::TableHeaderCell => "TH".to_string(),
            Semantic::TableCell => "TD".to_string(),
            Semantic::Link { .. } => "Link".to_string(),
            Semantic::Figure { .. } => "Figure".to_string(),
        })
    }

    fn carries_content(&self) -> bool {
        !matches!(
            self.semantic,
            Semantic::List | Semantic::ListItem | Semantic::Table | Semantic::TableRow
        )
    }

    fn structure(&self) -> Result<StructElem, ComposeError> {
        let mut element = StructElem::new(self.tag()?);
        if matches!(self.semantic, Semantic::List) {
            element = element.list_numbering(ListNumbering::None);
        }
        if matches!(self.semantic, Semantic::TableHeaderCell) {
            element = element.th_scope(ThScope::Column);
        }
        if let Semantic::Figure { alt } = &self.semantic {
            if alt.is_empty() {
                return Err(ComposeError::InvalidGeometry);
            }
            element = element.alt(alt);
        }
        for &(page, mcid) in &self.marks {
            element.push_content(page, mcid);
        }
        for &index in &self.annotation_indices {
            element.push_annotation(index);
        }
        let mut children = Vec::new();
        self.child.collect_structure(&mut children)?;
        for child in children {
            element.push_child(child);
        }
        Ok(element)
    }
}

impl Element for SemanticNode {
    fn measure(&mut self, available: Size, metrics: &Metrics) -> Result<Plan, ComposeError> {
        let _ = self.tag()?;
        self.child.measure(available, metrics)
    }

    fn draw(&mut self, context: &mut DrawCtx<'_>, space: Size) -> Result<(), ComposeError> {
        let tag = self.tag()?;
        let mark = if context.tagged && self.carries_content() {
            let mcid = *context.mcid_next;
            *context.mcid_next = mcid.checked_add(1).ok_or(ComposeError::NoProgress)?;
            context.content.begin_marked_content(&tag, mcid);
            *context.marked_depth += 1;
            Some((context.page, mcid))
        } else {
            None
        };
        self.child.draw(context, space)?;
        if let Some(mark) = mark {
            *context.marked_depth -= 1;
            context.content.end_marked_content();
            self.marks.push(mark);
        }
        if context.tagged
            && let Semantic::Link { uri, description } = &self.semantic
        {
            let bottom = context.page_height - context.origin.y - space.height;
            let index = context.annotations.len();
            context.annotations.push((
                context.page,
                AnnotationSpec::Link {
                    rect: [
                        context.origin.x,
                        bottom,
                        context.origin.x + space.width,
                        bottom + space.height,
                    ],
                    target: LinkTarget::Uri(uri.clone()),
                    contents: Some(description.clone()),
                },
            ));
            self.annotation_indices.push(index);
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.child.reset();
    }
}

pub(super) struct TableMeasure {
    plan: Plan,
    header_size: Option<Size>,
    rows: Vec<(usize, Size)>,
}

pub(super) struct TableNode {
    columns: Vec<RowWidth>,
    header: Option<RowNode>,
    rows: Vec<RowNode>,
    row_index: usize,
    measured: Option<TableMeasure>,
    resolved_widths: Option<(f64, Vec<f64>)>,
    invalid: bool,
}

impl TableNode {
    pub(super) fn new(
        columns: Vec<RowWidth>,
        header: Option<Vec<Option<Node>>>,
        rows: Vec<Vec<Option<Node>>>,
    ) -> Self {
        let invalid = header
            .as_ref()
            .is_some_and(|cells| cells.len() != columns.len())
            || rows.iter().any(|cells| cells.len() != columns.len());
        let make_row = |cells: Vec<Option<Node>>| {
            RowNode::new(
                columns
                    .iter()
                    .copied()
                    .zip(
                        cells
                            .into_iter()
                            .map(|cell| cell.unwrap_or_else(Node::empty_column)),
                    )
                    .collect(),
            )
        };
        Self {
            header: header.map(make_row),
            rows: rows.into_iter().map(make_row).collect(),
            columns,
            row_index: 0,
            measured: None,
            resolved_widths: None,
            invalid,
        }
    }

    fn validate(&self) -> Result<(), ComposeError> {
        if self.invalid
            || self.columns.is_empty()
            || self
                .header
                .as_ref()
                .is_some_and(|row| row.children.len() != self.columns.len())
            || self
                .rows
                .iter()
                .any(|row| row.children.len() != self.columns.len())
        {
            return Err(ComposeError::InvalidGeometry);
        }
        Ok(())
    }

    fn widths(&mut self, available: Size, metrics: &Metrics) -> Result<Vec<f64>, ComposeError> {
        self.validate()?;
        if let Some((width, resolved)) = &self.resolved_widths
            && (*width - available.width).abs() <= EPSILON
        {
            return Ok(resolved.clone());
        }
        let mut fixed = 0.0;
        let mut relative = 0.0;
        let mut automatic = vec![0.0f64; self.columns.len()];
        for (index, column) in self.columns.iter().copied().enumerate() {
            match column {
                RowWidth::Fixed(width) if width.is_finite() && width >= 0.0 => fixed += width,
                RowWidth::Relative(factor) if factor.is_finite() && factor > 0.0 => {
                    relative += factor;
                }
                RowWidth::Auto => {
                    let rows = self
                        .header
                        .iter_mut()
                        .chain(self.rows[self.row_index..].iter_mut());
                    for row in rows {
                        let child = &mut row.children[index].1;
                        let plan = child.measure(available, metrics)?;
                        if let Plan::Full(size) | Plan::Partial(size) = plan {
                            automatic[index] = automatic[index].max(size.width);
                        }
                        child.reset();
                    }
                }
                RowWidth::Fixed(_) | RowWidth::Relative(_) => {
                    return Err(ComposeError::InvalidGeometry);
                }
            }
        }
        let auto_total = automatic.iter().sum::<f64>();
        if fixed + auto_total > available.width + EPSILON {
            return Err(ComposeError::InvalidGeometry);
        }
        let remainder = (available.width - fixed - auto_total).max(0.0);
        let resolved = self
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| match *column {
                RowWidth::Fixed(width) => width,
                RowWidth::Relative(factor) => remainder * factor / relative,
                RowWidth::Auto => automatic[index],
            })
            .collect::<Vec<_>>();
        self.resolved_widths = Some((available.width, resolved.clone()));
        Ok(resolved)
    }
}

impl Element for TableNode {
    fn measure(&mut self, available: Size, metrics: &Metrics) -> Result<Plan, ComposeError> {
        if self.row_index >= self.rows.len() {
            self.measured = Some(TableMeasure {
                plan: Plan::Empty,
                header_size: None,
                rows: Vec::new(),
            });
            return Ok(Plan::Empty);
        }
        if !available.is_valid() {
            return Err(ComposeError::InvalidGeometry);
        }
        let widths = self.widths(available, metrics)?;
        let mut used = 0.0;
        let header_size = if let Some(header) = &mut self.header {
            header.reset();
            match header.measure_at_widths(available, &widths, metrics)? {
                Plan::Full(size) => {
                    used = size.height;
                    Some(size)
                }
                Plan::Empty => None,
                Plan::Partial(_) | Plan::Wrap => return Ok(Plan::Wrap),
            }
        } else {
            None
        };
        let mut planned_rows = Vec::new();
        for index in self.row_index..self.rows.len() {
            let remaining = (available.height - used).max(0.0);
            match self.rows[index].measure_at_widths(
                Size::new(available.width, remaining),
                &widths,
                metrics,
            )? {
                Plan::Full(size) => {
                    used += size.height;
                    planned_rows.push((index, size));
                }
                Plan::Empty => planned_rows.push((index, Size::default())),
                Plan::Partial(_) | Plan::Wrap => break,
            }
        }
        if planned_rows.is_empty() {
            if let Some(header) = &mut self.header {
                header.reset();
            }
            self.measured = None;
            return Ok(Plan::Wrap);
        }
        let complete = planned_rows
            .last()
            .is_some_and(|(index, _)| *index + 1 == self.rows.len());
        let size = Size::new(available.width, used);
        let plan = if complete {
            Plan::Full(size)
        } else {
            Plan::Partial(size)
        };
        self.measured = Some(TableMeasure {
            plan,
            header_size,
            rows: planned_rows,
        });
        Ok(plan)
    }

    fn draw(&mut self, context: &mut DrawCtx<'_>, space: Size) -> Result<(), ComposeError> {
        let Some(measure) = self.measured.take() else {
            return Err(ComposeError::MeasurementMismatch);
        };
        let expected = match measure.plan {
            Plan::Full(size) | Plan::Partial(size) => size,
            Plan::Empty | Plan::Wrap => return Err(ComposeError::MeasurementMismatch),
        };
        if expected != space {
            return Err(ComposeError::MeasurementMismatch);
        }
        let mut y = 0.0;
        if let (Some(header), Some(size)) = (&mut self.header, measure.header_size) {
            let was_tagged = context.tagged;
            let mut child_context = context.translated(0.0, y);
            let repeated = self.row_index > 0;
            if repeated && child_context.tagged {
                child_context.content.begin_artifact();
                child_context.tagged = false;
            }
            header.draw(&mut child_context, size)?;
            if repeated && was_tagged {
                child_context.content.end_marked_content();
            }
            header.reset();
            y += size.height;
        }
        for (index, size) in measure.rows {
            if size != Size::default() {
                let mut child_context = context.translated(0.0, y);
                self.rows[index].draw(&mut child_context, size)?;
                y += size.height;
            }
            self.row_index = index + 1;
        }
        context.trace.events.push(GeometryEvent {
            page: context.page,
            kind: "Table",
            bounds: Rect {
                origin: context.origin,
                size: space,
            },
            text: None,
        });
        Ok(())
    }

    fn reset(&mut self) {
        self.row_index = 0;
        self.measured = None;
        self.resolved_widths = None;
        if let Some(header) = &mut self.header {
            header.reset();
        }
        for row in &mut self.rows {
            row.reset();
        }
    }
}

pub(super) struct DecoratedMeasure {
    plan: Plan,
    child_size: Size,
    child_offset: Point,
}

pub(super) struct DecoratedNode {
    decoration: Decoration,
    child: Box<Node>,
    measured: Option<DecoratedMeasure>,
}

impl DecoratedNode {
    pub(super) fn new(decoration: Decoration, child: Node) -> Self {
        Self {
            decoration,
            child: Box::new(child),
            measured: None,
        }
    }

    fn constraints(&self, available: Size) -> Result<(Size, f64, f64), ComposeError> {
        let decoration = self.decoration;
        if !available.is_valid()
            || decoration
                .padding
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
            || decoration
                .width
                .is_some_and(|value| !value.is_finite() || value < 0.0)
            || decoration
                .height
                .is_some_and(|value| !value.is_finite() || value < 0.0)
            || decoration.border.is_some_and(|(width, color)| {
                !width.is_finite() || width < 0.0 || !color.is_valid()
            })
            || decoration.background.is_some_and(|color| !color.is_valid())
        {
            return Err(ComposeError::InvalidGeometry);
        }
        let outer_width = decoration.width.unwrap_or(available.width);
        let outer_height = decoration.height.unwrap_or(available.height);
        if outer_width > available.width + EPSILON || outer_height > available.height + EPSILON {
            return Err(ComposeError::InvalidGeometry);
        }
        let horizontal_padding = decoration.padding[1] + decoration.padding[3];
        let vertical_padding = decoration.padding[0] + decoration.padding[2];
        if horizontal_padding > outer_width + EPSILON || vertical_padding > outer_height + EPSILON {
            return Err(ComposeError::InvalidGeometry);
        }
        Ok((
            Size::new(
                (outer_width - horizontal_padding).max(0.0),
                (outer_height - vertical_padding).max(0.0),
            ),
            horizontal_padding,
            vertical_padding,
        ))
    }
}

impl Element for DecoratedNode {
    fn measure(&mut self, available: Size, metrics: &Metrics) -> Result<Plan, ComposeError> {
        let (inner, horizontal_padding, vertical_padding) = self.constraints(available)?;
        let child_plan = self.child.measure(inner, metrics)?;
        let child_size = match child_plan {
            Plan::Empty => {
                self.measured = Some(DecoratedMeasure {
                    plan: Plan::Empty,
                    child_size: Size::default(),
                    child_offset: Point::default(),
                });
                return Ok(Plan::Empty);
            }
            Plan::Wrap => {
                self.measured = None;
                return Ok(Plan::Wrap);
            }
            Plan::Full(size) | Plan::Partial(size) => size,
        };
        if !child_size.is_valid()
            || child_size.width > inner.width + EPSILON
            || child_size.height > inner.height + EPSILON
        {
            return Err(ComposeError::MeasurementMismatch);
        }
        let width = if self.decoration.width.is_some() || self.decoration.extend_width {
            inner.width + horizontal_padding
        } else {
            child_size.width + horizontal_padding
        };
        let height = if self.decoration.height.is_some() || self.decoration.extend_height {
            inner.height + vertical_padding
        } else {
            child_size.height + vertical_padding
        };
        let spare_x = (width - horizontal_padding - child_size.width).max(0.0);
        let spare_y = (height - vertical_padding - child_size.height).max(0.0);
        let x = self.decoration.padding[3]
            + match self.decoration.horizontal {
                HorizontalAlign::Left => 0.0,
                HorizontalAlign::Center => spare_x / 2.0,
                HorizontalAlign::Right => spare_x,
            };
        let y = self.decoration.padding[0]
            + match self.decoration.vertical {
                VerticalAlign::Top => 0.0,
                VerticalAlign::Center => spare_y / 2.0,
                VerticalAlign::Bottom => spare_y,
            };
        let size = Size::new(width, height);
        let plan = if matches!(child_plan, Plan::Partial(_)) {
            Plan::Partial(size)
        } else {
            Plan::Full(size)
        };
        self.measured = Some(DecoratedMeasure {
            plan,
            child_size,
            child_offset: Point { x, y },
        });
        Ok(plan)
    }

    fn draw(&mut self, context: &mut DrawCtx<'_>, space: Size) -> Result<(), ComposeError> {
        let Some(measure) = self.measured.take() else {
            return Err(ComposeError::MeasurementMismatch);
        };
        let expected = match measure.plan {
            Plan::Full(size) | Plan::Partial(size) => size,
            Plan::Empty | Plan::Wrap => return Err(ComposeError::MeasurementMismatch),
        };
        if expected != space {
            return Err(ComposeError::MeasurementMismatch);
        }
        let pdf_y = context.page_height - context.origin.y - space.height;
        if let Some(color) = self.decoration.background {
            if context.tagged {
                context.content.begin_artifact();
            }
            context.content.save();
            context
                .content
                .set_fill_rgb(color.red, color.green, color.blue)
                .rect(context.origin.x, pdf_y, space.width, space.height)
                .fill();
            context.content.restore();
            if context.tagged {
                context.content.end_marked_content();
            }
        }
        let mut child_context = context.translated(measure.child_offset.x, measure.child_offset.y);
        self.child.draw(&mut child_context, measure.child_size)?;
        if let Some((width, color)) = self.decoration.border {
            if context.tagged {
                context.content.begin_artifact();
            }
            context.content.save();
            context
                .content
                .set_line_width(width)
                .set_stroke_rgb(color.red, color.green, color.blue)
                .rect(context.origin.x, pdf_y, space.width, space.height)
                .stroke();
            context.content.restore();
            if context.tagged {
                context.content.end_marked_content();
            }
        }
        context.trace.events.push(GeometryEvent {
            page: context.page,
            kind: "Decorated",
            bounds: Rect {
                origin: context.origin,
                size: space,
            },
            text: None,
        });
        Ok(())
    }

    fn reset(&mut self) {
        self.measured = None;
        self.child.reset();
    }
}

pub(super) struct TextMeasure {
    plan: Plan,
    lines: Vec<String>,
    width: f64,
}

pub(super) struct TextNode {
    text: String,
    style: TextStyle,
    consumed: usize,
    complete: bool,
    measured: Option<TextMeasure>,
    page: usize,
    pages: usize,
}

impl TextNode {
    pub(super) fn new(text: &str, style: TextStyle) -> Self {
        Self {
            text: text.to_string(),
            style,
            consumed: 0,
            complete: false,
            measured: None,
            page: 1,
            pages: 1,
        }
    }

    fn rendered_text(&self) -> String {
        self.text
            .replace("{page}", &self.page.to_string())
            .replace("{pages}", &self.pages.to_string())
    }

    fn all_lines(&self, metrics: &dyn FontMetrics, width: f64) -> Vec<String> {
        self.rendered_text()
            .split('\n')
            .flat_map(|paragraph| wrap_paragraph_with(metrics, paragraph, self.style.size, width))
            .collect()
    }
}

impl Element for TextNode {
    fn measure(&mut self, available: Size, metrics: &Metrics) -> Result<Plan, ComposeError> {
        if !available.is_valid()
            || !self.style.size.is_finite()
            || self.style.size <= 0.0
            || !self.style.leading.is_finite()
            || self.style.leading <= 0.0
        {
            return Err(ComposeError::InvalidGeometry);
        }
        if self.complete {
            self.measured = Some(TextMeasure {
                plan: Plan::Empty,
                lines: Vec::new(),
                width: 0.0,
            });
            return Ok(Plan::Empty);
        }
        let font = metrics.for_resource(&self.style.font_resource)?;
        let lines = self.all_lines(font.as_ref(), available.width);
        if self.consumed >= lines.len() {
            self.complete = true;
            return Ok(Plan::Empty);
        }
        let capacity = (available.height / self.style.leading).floor() as usize;
        if capacity == 0 {
            self.measured = Some(TextMeasure {
                plan: Plan::Wrap,
                lines: Vec::new(),
                width: 0.0,
            });
            return Ok(Plan::Wrap);
        }
        let count = capacity.min(lines.len() - self.consumed);
        let selected = lines[self.consumed..self.consumed + count].to_vec();
        let width = selected
            .iter()
            .filter_map(|line| font.width(line, self.style.size))
            .fold(0.0, f64::max)
            .min(available.width);
        let size = Size::new(width, count as f64 * self.style.leading);
        let plan = if self.consumed + count == lines.len() {
            Plan::Full(size)
        } else {
            Plan::Partial(size)
        };
        self.measured = Some(TextMeasure {
            plan,
            lines: selected,
            width,
        });
        Ok(plan)
    }

    fn draw(&mut self, context: &mut DrawCtx<'_>, space: Size) -> Result<(), ComposeError> {
        let Some(measure) = self.measured.take() else {
            return Err(ComposeError::MeasurementMismatch);
        };
        let expected = match measure.plan {
            Plan::Full(size) | Plan::Partial(size) => size,
            Plan::Empty | Plan::Wrap => return Err(ComposeError::MeasurementMismatch),
        };
        if expected != space || measure.width != space.width {
            return Err(ComposeError::MeasurementMismatch);
        }
        let artifact = context.tagged && *context.marked_depth == 0;
        if artifact {
            context.content.begin_artifact();
        }
        let mut drawn = Vec::new();
        for (line_index, line) in measure.lines.iter().enumerate() {
            let baseline_from_top = self.style.size + line_index as f64 * self.style.leading;
            let pdf_y = context.page_height - context.origin.y - baseline_from_top;
            context.content.begin_text();
            context
                .content
                .set_font(&self.style.font_resource, self.style.size);
            context
                .content
                .set_text_matrix(1.0, 0.0, 0.0, 1.0, context.origin.x, pdf_y);
            match context.metrics.glyphs(&self.style.font_resource, line)? {
                Some(glyphs) => context.content.show_glyphs(&glyphs),
                None => context.content.show_text(&pdf_fonts::winansi_encode(line)),
            };
            context.content.end_text();
            drawn.push(line.as_str());
        }
        self.consumed += measure.lines.len();
        self.complete = matches!(measure.plan, Plan::Full(_));
        if artifact {
            context.content.end_marked_content();
        }
        context.trace.events.push(GeometryEvent {
            page: context.page,
            kind: "Text",
            bounds: Rect {
                origin: context.origin,
                size: space,
            },
            text: Some(drawn.join("\n")),
        });
        Ok(())
    }

    fn reset(&mut self) {
        self.consumed = 0;
        self.complete = false;
        self.measured = None;
    }
}

pub(super) struct DrawCtx<'a> {
    pub(super) content: &'a mut Content,
    pub(super) metrics: &'a Metrics<'a>,
    pub(super) trace: &'a mut GeometryTrace,
    pub(super) page: usize,
    pub(super) page_height: f64,
    pub(super) origin: Point,
    pub(super) images: &'a mut Vec<(String, ImageXObject)>,
    pub(super) tagged: bool,
    pub(super) mcid_next: &'a mut u32,
    pub(super) annotations: &'a mut Vec<(usize, AnnotationSpec)>,
    pub(super) marked_depth: &'a mut usize,
}

impl DrawCtx<'_> {
    pub(super) fn translated(&mut self, dx: f64, dy: f64) -> DrawCtx<'_> {
        DrawCtx {
            content: self.content,
            metrics: self.metrics,
            trace: self.trace,
            page: self.page,
            page_height: self.page_height,
            origin: Point {
                x: self.origin.x + dx,
                y: self.origin.y + dy,
            },
            images: self.images,
            tagged: self.tagged,
            mcid_next: self.mcid_next,
            annotations: self.annotations,
            marked_depth: self.marked_depth,
        }
    }
}
