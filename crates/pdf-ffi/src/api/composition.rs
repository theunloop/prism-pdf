use super::*;

// ---------------------------------------------------------------------------------------------

static NEXT_COMPOSITION_ID: AtomicU64 = AtomicU64::new(1);

/// Page geometry for declarative composition, in PDF points.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PrismPdfCompositionPageStyle {
    /// Page width.
    pub width: f64,
    /// Page height.
    pub height: f64,
    /// Left content margin.
    pub margin_left: f64,
    /// Right content margin.
    pub margin_right: f64,
    /// Top content margin.
    pub margin_top: f64,
    /// Bottom content margin.
    pub margin_bottom: f64,
}

impl From<PrismPdfCompositionPageStyle> for PageStyle {
    fn from(style: PrismPdfCompositionPageStyle) -> Self {
        Self {
            size: [style.width, style.height],
            margins: [
                style.margin_left,
                style.margin_right,
                style.margin_top,
                style.margin_bottom,
            ],
        }
    }
}

/// Text styling for declarative composition.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PrismPdfCompositionTextStyle {
    /// Font size in points.
    pub size: f64,
    /// Baseline-to-baseline line spacing in points.
    pub leading: f64,
}

/// RGB colour for declarative composition; every component must be in the inclusive range 0–1.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PrismPdfCompositionColor {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
}

/// Horizontal alignment inside a constrained composition box.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub enum PrismPdfCompositionHorizontalAlign {
    Left = 0,
    Center = 1,
    Right = 2,
}

/// Vertical alignment inside a constrained composition box.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub enum PrismPdfCompositionVerticalAlign {
    Top = 0,
    Center = 1,
    Bottom = 2,
}

/// Logical structure roles without associated string or numeric data (§14.7–§14.8).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub enum PrismPdfCompositionSemantic {
    Paragraph = 0,
    List = 1,
    ListItem = 2,
    ListLabel = 3,
    ListBody = 4,
    Table = 5,
    TableRow = 6,
    TableHeaderCell = 7,
    TableCell = 8,
}

/// Image scaling policy inside the requested composition box (§8.9).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub enum PrismPdfCompositionImageSizing {
    Fit = 0,
    Fill = 1,
    Exact = 2,
}

#[derive(Clone, Copy)]
pub(crate) enum CompositionDraftRowWidth {
    Fixed(f64),
    Relative(f64),
    Auto,
}

#[derive(Clone, Copy)]
pub(crate) enum CompositionDraftDecoration {
    Padding(f64),
    Align(LayoutHorizontalAlign, LayoutVerticalAlign),
    Width(f64),
    Height(f64),
    Extend,
    Border(f64, LayoutColor),
    Background(LayoutColor),
}

#[derive(Clone)]
pub(crate) enum CompositionDraftNode {
    Empty,
    Column {
        spacing: f64,
        children: Vec<usize>,
    },
    Row {
        children: Vec<(CompositionDraftRowWidth, usize)>,
    },
    Decorated {
        decoration: CompositionDraftDecoration,
        child: usize,
    },
    Semantic {
        semantic: Semantic,
        child: usize,
    },
    Table {
        columns: Vec<CompositionDraftRowWidth>,
        header: Option<usize>,
        rows: Vec<usize>,
    },
    TableRow {
        cells: Vec<usize>,
    },
    Image {
        image: Image,
        sizing: ImageSizing,
    },
    Text {
        text: String,
        style: PrismPdfCompositionTextStyle,
    },
    PageBreak,
}

#[derive(Clone)]
pub(crate) struct CompositionDraftSlot {
    generation: u64,
    node: CompositionDraftNode,
}

#[derive(Clone)]
pub(crate) struct CompositionDraftPage {
    style: PrismPdfCompositionPageStyle,
    root: usize,
    header: Option<usize>,
    footer: Option<usize>,
}

pub(crate) struct CompositionArena {
    tree_id: u64,
    alive: bool,
    finalised: bool,
    slots: Vec<CompositionDraftSlot>,
    pages: Vec<CompositionDraftPage>,
    lang: Option<String>,
}

/// Opaque declarative-composition handle. Build is one-way finalisation.
pub struct PrismPdfComposition(pub(crate) Arc<Mutex<CompositionArena>>);

/// Opaque scoped container handle. Freeing it never frees its composition tree.
pub struct PrismPdfCompositionContainer {
    arena: Arc<Mutex<CompositionArena>>,
    tree_id: u64,
    slot: usize,
    generation: u64,
}

pub(crate) fn composition_status(error: prismpdf::ComposeError) -> PrismPdfStatus {
    match error {
        prismpdf::ComposeError::MissingFont(_) => PrismPdfStatus::Layout,
        prismpdf::ComposeError::InvalidFont
        | prismpdf::ComposeError::InvalidGeometry
        | prismpdf::ComposeError::OverTallElement
        | prismpdf::ComposeError::NoProgress
        | prismpdf::ComposeError::MeasurementMismatch => PrismPdfStatus::Layout,
    }
}

pub(crate) fn allocate_slot(arena: &mut CompositionArena) -> usize {
    let slot = arena.slots.len();
    arena.slots.push(CompositionDraftSlot {
        generation: 1,
        node: CompositionDraftNode::Empty,
    });
    slot
}

pub(crate) fn container_handle(
    arena: &Arc<Mutex<CompositionArena>>,
    tree_id: u64,
    slot: usize,
    generation: u64,
) -> *mut PrismPdfCompositionContainer {
    Box::into_raw(Box::new(PrismPdfCompositionContainer {
        arena: Arc::clone(arena),
        tree_id,
        slot,
        generation,
    }))
}

pub(crate) fn with_live_container(
    container: *mut PrismPdfCompositionContainer,
    body: impl FnOnce(&mut CompositionArena, usize) -> PrismPdfStatus,
) -> PrismPdfStatus {
    if container.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let handle = unsafe { &*container };
    let Ok(mut arena) = handle.arena.lock() else {
        return PrismPdfStatus::Internal;
    };
    if !arena.alive
        || arena.finalised
        || arena.tree_id != handle.tree_id
        || arena
            .slots
            .get(handle.slot)
            .is_none_or(|slot| slot.generation != handle.generation)
    {
        return PrismPdfStatus::InvalidUse;
    }
    body(&mut arena, handle.slot)
}

pub(crate) fn fill_slot(
    container: *mut PrismPdfCompositionContainer,
    node: CompositionDraftNode,
) -> PrismPdfStatus {
    with_live_container(container, |arena, index| {
        let Some(slot) = arena.slots.get_mut(index) else {
            return PrismPdfStatus::InvalidUse;
        };
        if !matches!(slot.node, CompositionDraftNode::Empty) {
            return PrismPdfStatus::InvalidUse;
        }
        let Some(generation) = slot.generation.checked_add(1) else {
            return PrismPdfStatus::Internal;
        };
        slot.generation = generation;
        slot.node = node;
        PrismPdfStatus::Ok
    })
}

pub(crate) fn emit_draft_node(
    container: &mut ComposeContainer<'_>,
    index: usize,
    slots: &[CompositionDraftSlot],
) {
    let Some(slot) = slots.get(index) else {
        return;
    };
    match &slot.node {
        CompositionDraftNode::Empty => container.column(|_| {}),
        CompositionDraftNode::Column { spacing, children } => container.column(|column| {
            column.spacing(*spacing);
            for child in children {
                emit_draft_node(&mut column.item(), *child, slots);
            }
        }),
        CompositionDraftNode::Row { children } => container.row(|row| {
            for (width, child) in children {
                let mut item = match width {
                    CompositionDraftRowWidth::Fixed(points) => row.fixed(*points),
                    CompositionDraftRowWidth::Relative(factor) => row.relative(*factor),
                    CompositionDraftRowWidth::Auto => row.auto(),
                };
                emit_draft_node(&mut item, *child, slots);
            }
        }),
        CompositionDraftNode::Decorated { decoration, child } => match decoration {
            CompositionDraftDecoration::Padding(points) => {
                container.padding(*points, |item| emit_draft_node(item, *child, slots));
            }
            CompositionDraftDecoration::Align(horizontal, vertical) => {
                container.align(*horizontal, *vertical, |item| {
                    emit_draft_node(item, *child, slots)
                })
            }
            CompositionDraftDecoration::Width(points) => {
                container.width(*points, |item| emit_draft_node(item, *child, slots));
            }
            CompositionDraftDecoration::Height(points) => {
                container.height(*points, |item| emit_draft_node(item, *child, slots));
            }
            CompositionDraftDecoration::Extend => {
                container.extend(|item| emit_draft_node(item, *child, slots));
            }
            CompositionDraftDecoration::Border(width, color) => {
                container.border(*width, *color, |item| emit_draft_node(item, *child, slots));
            }
            CompositionDraftDecoration::Background(color) => {
                container.background(*color, |item| emit_draft_node(item, *child, slots));
            }
        },
        CompositionDraftNode::Semantic { semantic, child } => {
            container.semantic(semantic.clone(), |item| {
                emit_draft_node(item, *child, slots)
            });
        }
        CompositionDraftNode::Table {
            columns,
            header,
            rows,
        } => container.table(|table| {
            for width in columns {
                match width {
                    CompositionDraftRowWidth::Fixed(points) => table.fixed_column(*points),
                    CompositionDraftRowWidth::Relative(factor) => table.relative_column(*factor),
                    CompositionDraftRowWidth::Auto => table.automatic_column(),
                }
            }
            if let Some(header) = header
                && let Some(CompositionDraftSlot {
                    node: CompositionDraftNode::TableRow { cells },
                    ..
                }) = slots.get(*header)
            {
                table.header(|row| {
                    for cell in cells {
                        emit_draft_node(&mut row.cell(), *cell, slots);
                    }
                });
            }
            for row_index in rows {
                if let Some(CompositionDraftSlot {
                    node: CompositionDraftNode::TableRow { cells },
                    ..
                }) = slots.get(*row_index)
                {
                    table.row(|row| {
                        for cell in cells {
                            emit_draft_node(&mut row.cell(), *cell, slots);
                        }
                    });
                }
            }
        }),
        CompositionDraftNode::TableRow { .. } => container.column(|_| {}),
        CompositionDraftNode::Image { image, sizing } => container.image(image, *sizing),
        CompositionDraftNode::Text { text, style } => {
            container.text(
                text,
                TextStyle::new().size(style.size).leading(style.leading),
            );
        }
        CompositionDraftNode::PageBreak => container.page_break(),
    }
}

pub(crate) fn build_draft(arena: &CompositionArena) -> Result<Vec<u8>, prismpdf::ComposeError> {
    let mut composition = Composition::new();
    if let Some(lang) = &arena.lang {
        composition = composition.tagged(lang);
    }
    for draft_page in &arena.pages {
        let slots = &arena.slots;
        let root = draft_page.root;
        let header = draft_page.header;
        let footer = draft_page.footer;
        composition = composition.page(draft_page.style.into(), |page| {
            if let Some(header) = header {
                emit_draft_node(&mut page.header(), header, slots);
            }
            emit_draft_node(&mut page.content(), root, slots);
            if let Some(footer) = footer {
                emit_draft_node(&mut page.footer(), footer, slots);
            }
        });
    }
    composition
        .build()
        .map(prismpdf::ComposedDocument::into_pdf)
}

/// Create an empty declarative composition.
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_composition_new() -> *mut PrismPdfComposition {
    guard_ptr(|| {
        let tree_id = NEXT_COMPOSITION_ID.fetch_add(1, Ordering::Relaxed);
        Box::into_raw(Box::new(PrismPdfComposition(Arc::new(Mutex::new(
            CompositionArena {
                tree_id,
                alive: true,
                finalised: false,
                slots: Vec::new(),
                pages: Vec::new(),
                lang: None,
            },
        )))))
    })
}

/// Release a composition. Surviving container handles become invalid but remain safe to free.
///
/// # Safety
/// `composition` must be null or a live handle returned by [`prismpdf_composition_new`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_free(composition: *mut PrismPdfComposition) {
    if composition.is_null() {
        return;
    }
    let _ = guard(|| {
        let composition = unsafe { Box::from_raw(composition) };
        if let Ok(mut arena) = composition.0.lock() {
            arena.alive = false;
        }
        drop(composition);
        PrismPdfStatus::Ok
    });
}

/// Release a scoped container handle. The owned node remains in its composition.
///
/// # Safety
/// `container` must be null or a live container handle returned by this API.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_free(
    container: *mut PrismPdfCompositionContainer,
) {
    if container.is_null() {
        return;
    }
    let _ = guard(|| {
        drop(unsafe { Box::from_raw(container) });
        PrismPdfStatus::Ok
    });
}

/// Add a page and return its empty content slot.
///
/// # Safety
/// All pointers must be live/writable. Free `*out_content` with
/// [`prismpdf_composition_container_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_add_page(
    composition: *mut PrismPdfComposition,
    style: *const PrismPdfCompositionPageStyle,
    out_content: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    if composition.is_null() || style.is_null() || out_content.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_content = std::ptr::null_mut() };
    guard(|| {
        let composition = unsafe { &*composition };
        let Ok(mut arena) = composition.0.lock() else {
            return PrismPdfStatus::Internal;
        };
        if !arena.alive || arena.finalised {
            return PrismPdfStatus::InvalidUse;
        }
        let root = allocate_slot(&mut arena);
        let generation = arena.slots[root].generation;
        let tree_id = arena.tree_id;
        arena.pages.push(CompositionDraftPage {
            style: unsafe { *style },
            root,
            header: None,
            footer: None,
        });
        unsafe {
            *out_content = container_handle(&composition.0, tree_id, root, generation);
        }
        PrismPdfStatus::Ok
    })
}

pub(crate) fn composition_page_repeating_slot(
    composition: *mut PrismPdfComposition,
    page_index: usize,
    header: bool,
    out_content: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    if composition.is_null() || out_content.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_content = std::ptr::null_mut() };
    guard(|| {
        let composition = unsafe { &*composition };
        let Ok(mut arena) = composition.0.lock() else {
            return PrismPdfStatus::Internal;
        };
        if !arena.alive || arena.finalised {
            return PrismPdfStatus::InvalidUse;
        }
        let Some(page) = arena.pages.get(page_index) else {
            return PrismPdfStatus::NotFound;
        };
        if (header && page.header.is_some()) || (!header && page.footer.is_some()) {
            return PrismPdfStatus::InvalidUse;
        }
        let slot = allocate_slot(&mut arena);
        if header {
            arena.pages[page_index].header = Some(slot);
        } else {
            arena.pages[page_index].footer = Some(slot);
        }
        let tree_id = arena.tree_id;
        let generation = arena.slots[slot].generation;
        unsafe {
            *out_content = container_handle(&composition.0, tree_id, slot, generation);
        }
        PrismPdfStatus::Ok
    })
}

/// Add a header tree repeated on every physical page produced by a page design.
///
/// Text nodes may contain `{page}` and `{pages}` placeholders.
///
/// # Safety
/// `composition` must be live and `out_header` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_page_set_header(
    composition: *mut PrismPdfComposition,
    page_index: usize,
    out_header: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    composition_page_repeating_slot(composition, page_index, true, out_header)
}

/// Add a footer tree repeated on every physical page produced by a page design.
///
/// Text nodes may contain `{page}` and `{pages}` placeholders.
///
/// # Safety
/// `composition` must be live and `out_footer` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_page_set_footer(
    composition: *mut PrismPdfComposition,
    page_index: usize,
    out_footer: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    composition_page_repeating_slot(composition, page_index, false, out_footer)
}

/// Enable tagged-PDF output and set the document language (for example `en-US`).
///
/// # Safety
/// `composition` must be live and `lang` a valid NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_set_tagged_language(
    composition: *mut PrismPdfComposition,
    lang: *const c_char,
) -> PrismPdfStatus {
    if composition.is_null() || lang.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let Some(lang) = (unsafe { utf8(lang) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let composition = unsafe { &*composition };
        let Ok(mut arena) = composition.0.lock() else {
            return PrismPdfStatus::Internal;
        };
        if !arena.alive || arena.finalised {
            return PrismPdfStatus::InvalidUse;
        }
        arena.lang = Some(lang.to_string());
        PrismPdfStatus::Ok
    })
}

/// Fill an empty slot with a column and return a handle used to append child slots.
///
/// # Safety
/// `container` and `out_column` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_column(
    container: *mut PrismPdfCompositionContainer,
    spacing: f64,
    out_column: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    if out_column.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_column = std::ptr::null_mut() };
    if container.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let handle = unsafe { &*container };
    let arena_ref = Arc::clone(&handle.arena);
    let tree_id = handle.tree_id;
    let slot_index = handle.slot;
    let Some(generation) = handle.generation.checked_add(1) else {
        return PrismPdfStatus::Internal;
    };
    guard(|| {
        let status = fill_slot(
            container,
            CompositionDraftNode::Column {
                spacing,
                children: Vec::new(),
            },
        );
        if status == PrismPdfStatus::Ok {
            unsafe {
                *out_column = container_handle(&arena_ref, tree_id, slot_index, generation);
            }
        }
        status
    })
}

/// Append an empty child slot to a column.
///
/// # Safety
/// `column` and `out_child` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_column_add_item(
    column: *mut PrismPdfCompositionContainer,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    if out_child.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_child = std::ptr::null_mut() };
    guard(|| {
        let mut result = None;
        let status = with_live_container(column, |arena, index| {
            let child = allocate_slot(arena);
            let generation = arena.slots[child].generation;
            let tree_id = arena.tree_id;
            let Some(slot) = arena.slots.get_mut(index) else {
                return PrismPdfStatus::InvalidUse;
            };
            let CompositionDraftNode::Column { children, .. } = &mut slot.node else {
                return PrismPdfStatus::InvalidUse;
            };
            children.push(child);
            result = Some((tree_id, child, generation));
            PrismPdfStatus::Ok
        });
        if let Some((tree_id, child, generation)) = result {
            let arena = unsafe { &(*column).arena };
            unsafe { *out_child = container_handle(arena, tree_id, child, generation) };
        }
        status
    })
}

/// Fill an empty slot with a row and return a handle used to append child slots.
///
/// # Safety
/// `container` and `out_row` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_row(
    container: *mut PrismPdfCompositionContainer,
    out_row: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    if out_row.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_row = std::ptr::null_mut() };
    if container.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let handle = unsafe { &*container };
    let arena = Arc::clone(&handle.arena);
    let (tree_id, slot) = (handle.tree_id, handle.slot);
    let Some(generation) = handle.generation.checked_add(1) else {
        return PrismPdfStatus::Internal;
    };
    guard(|| {
        let status = fill_slot(
            container,
            CompositionDraftNode::Row {
                children: Vec::new(),
            },
        );
        if status == PrismPdfStatus::Ok {
            unsafe { *out_row = container_handle(&arena, tree_id, slot, generation) };
        }
        status
    })
}

pub(crate) fn row_add_item(
    row: *mut PrismPdfCompositionContainer,
    width: CompositionDraftRowWidth,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    if out_child.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_child = std::ptr::null_mut() };
    guard(|| {
        let mut result = None;
        let status = with_live_container(row, |arena, index| {
            if !matches!(arena.slots[index].node, CompositionDraftNode::Row { .. }) {
                return PrismPdfStatus::InvalidUse;
            }
            let child = allocate_slot(arena);
            let generation = arena.slots[child].generation;
            let tree_id = arena.tree_id;
            let CompositionDraftNode::Row { children } = &mut arena.slots[index].node else {
                return PrismPdfStatus::Internal;
            };
            children.push((width, child));
            result = Some((tree_id, child, generation));
            PrismPdfStatus::Ok
        });
        if let Some((tree_id, child, generation)) = result {
            let arena = unsafe { &(*row).arena };
            unsafe { *out_child = container_handle(arena, tree_id, child, generation) };
        }
        status
    })
}

/// Append an exact-width child to a row.
///
/// # Safety
/// `row` and `out_child` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_row_add_fixed(
    row: *mut PrismPdfCompositionContainer,
    width: f64,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    row_add_item(row, CompositionDraftRowWidth::Fixed(width), out_child)
}

/// Append a child receiving a weighted share of remaining row width.
///
/// # Safety
/// `row` and `out_child` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_row_add_relative(
    row: *mut PrismPdfCompositionContainer,
    factor: f64,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    row_add_item(row, CompositionDraftRowWidth::Relative(factor), out_child)
}

/// Append a naturally sized child to a row.
///
/// # Safety
/// `row` and `out_child` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_row_add_auto(
    row: *mut PrismPdfCompositionContainer,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    row_add_item(row, CompositionDraftRowWidth::Auto, out_child)
}

/// Fill an empty slot with a paginating table and return its editor handle.
///
/// # Safety
/// `container` and `out_table` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_table(
    container: *mut PrismPdfCompositionContainer,
    out_table: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    if container.is_null() || out_table.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_table = std::ptr::null_mut() };
    let handle = unsafe { &*container };
    let arena = Arc::clone(&handle.arena);
    let (tree_id, slot) = (handle.tree_id, handle.slot);
    let Some(generation) = handle.generation.checked_add(1) else {
        return PrismPdfStatus::Internal;
    };
    guard(|| {
        let status = fill_slot(
            container,
            CompositionDraftNode::Table {
                columns: Vec::new(),
                header: None,
                rows: Vec::new(),
            },
        );
        if status == PrismPdfStatus::Ok {
            unsafe { *out_table = container_handle(&arena, tree_id, slot, generation) };
        }
        status
    })
}

pub(crate) fn table_add_column(
    table: *mut PrismPdfCompositionContainer,
    width: CompositionDraftRowWidth,
) -> PrismPdfStatus {
    with_live_container(table, |arena, index| {
        let CompositionDraftNode::Table { columns, .. } = &mut arena.slots[index].node else {
            return PrismPdfStatus::InvalidUse;
        };
        columns.push(width);
        PrismPdfStatus::Ok
    })
}

/// Add an exact-width table column.
///
/// # Safety
/// `table` must be a live table editor handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_table_add_fixed_column(
    table: *mut PrismPdfCompositionContainer,
    width: f64,
) -> PrismPdfStatus {
    guard(|| table_add_column(table, CompositionDraftRowWidth::Fixed(width)))
}

/// Add a table column receiving a weighted share of remaining width.
///
/// # Safety
/// `table` must be a live table editor handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_table_add_relative_column(
    table: *mut PrismPdfCompositionContainer,
    factor: f64,
) -> PrismPdfStatus {
    guard(|| table_add_column(table, CompositionDraftRowWidth::Relative(factor)))
}

/// Add a naturally sized table column.
///
/// # Safety
/// `table` must be a live table editor handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_table_add_auto_column(
    table: *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    guard(|| table_add_column(table, CompositionDraftRowWidth::Auto))
}

pub(crate) fn table_add_row(
    table: *mut PrismPdfCompositionContainer,
    header: bool,
    out_row: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    if out_row.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_row = std::ptr::null_mut() };
    guard(|| {
        let mut result = None;
        let status = with_live_container(table, |arena, index| {
            let CompositionDraftNode::Table {
                header: current_header,
                ..
            } = &arena.slots[index].node
            else {
                return PrismPdfStatus::InvalidUse;
            };
            if header && current_header.is_some() {
                return PrismPdfStatus::InvalidUse;
            }
            let row = allocate_slot(arena);
            arena.slots[row].node = CompositionDraftNode::TableRow { cells: Vec::new() };
            let generation = arena.slots[row].generation;
            let tree_id = arena.tree_id;
            let CompositionDraftNode::Table {
                header: current_header,
                rows,
                ..
            } = &mut arena.slots[index].node
            else {
                return PrismPdfStatus::Internal;
            };
            if header {
                *current_header = Some(row);
            } else {
                rows.push(row);
            }
            result = Some((tree_id, row, generation));
            PrismPdfStatus::Ok
        });
        if let Some((tree_id, row, generation)) = result {
            let arena = unsafe { &(*table).arena };
            unsafe { *out_row = container_handle(arena, tree_id, row, generation) };
        }
        status
    })
}

/// Define the table header row, repeated on each table fragment.
///
/// # Safety
/// `table` and `out_row` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_table_set_header(
    table: *mut PrismPdfCompositionContainer,
    out_row: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    table_add_row(table, true, out_row)
}

/// Append a table body row.
///
/// # Safety
/// `table` and `out_row` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_table_add_row(
    table: *mut PrismPdfCompositionContainer,
    out_row: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    table_add_row(table, false, out_row)
}

/// Append an empty cell to a table row.
///
/// # Safety
/// `row` and `out_cell` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_table_row_add_cell(
    row: *mut PrismPdfCompositionContainer,
    out_cell: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    if out_cell.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_cell = std::ptr::null_mut() };
    guard(|| {
        let mut result = None;
        let status = with_live_container(row, |arena, index| {
            if !matches!(
                arena.slots[index].node,
                CompositionDraftNode::TableRow { .. }
            ) {
                return PrismPdfStatus::InvalidUse;
            }
            let cell = allocate_slot(arena);
            let generation = arena.slots[cell].generation;
            let tree_id = arena.tree_id;
            let CompositionDraftNode::TableRow { cells } = &mut arena.slots[index].node else {
                return PrismPdfStatus::Internal;
            };
            cells.push(cell);
            result = Some((tree_id, cell, generation));
            PrismPdfStatus::Ok
        });
        if let Some((tree_id, cell, generation)) = result {
            let arena = unsafe { &(*row).arena };
            unsafe { *out_cell = container_handle(arena, tree_id, cell, generation) };
        }
        status
    })
}

pub(crate) fn decorate_slot(
    container: *mut PrismPdfCompositionContainer,
    decoration: CompositionDraftDecoration,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    if container.is_null() || out_child.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_child = std::ptr::null_mut() };
    guard(|| {
        let mut result = None;
        let status = with_live_container(container, |arena, index| {
            if !matches!(arena.slots[index].node, CompositionDraftNode::Empty) {
                return PrismPdfStatus::InvalidUse;
            }
            let child = allocate_slot(arena);
            let child_generation = arena.slots[child].generation;
            let Some(parent_generation) = arena.slots[index].generation.checked_add(1) else {
                return PrismPdfStatus::Internal;
            };
            arena.slots[index].generation = parent_generation;
            arena.slots[index].node = CompositionDraftNode::Decorated { decoration, child };
            result = Some((arena.tree_id, child, child_generation));
            PrismPdfStatus::Ok
        });
        if let Some((tree_id, child, generation)) = result {
            let arena = unsafe { &(*container).arena };
            unsafe { *out_child = container_handle(arena, tree_id, child, generation) };
        }
        status
    })
}

pub(crate) fn semantic_slot(
    container: *mut PrismPdfCompositionContainer,
    semantic: Semantic,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    if container.is_null() || out_child.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_child = std::ptr::null_mut() };
    guard(|| {
        let mut result = None;
        let status = with_live_container(container, |arena, index| {
            if !matches!(arena.slots[index].node, CompositionDraftNode::Empty) {
                return PrismPdfStatus::InvalidUse;
            }
            let child = allocate_slot(arena);
            let child_generation = arena.slots[child].generation;
            let Some(parent_generation) = arena.slots[index].generation.checked_add(1) else {
                return PrismPdfStatus::Internal;
            };
            arena.slots[index].generation = parent_generation;
            arena.slots[index].node = CompositionDraftNode::Semantic { semantic, child };
            result = Some((arena.tree_id, child, child_generation));
            PrismPdfStatus::Ok
        });
        if let Some((tree_id, child, generation)) = result {
            let arena = unsafe { &(*container).arena };
            unsafe { *out_child = container_handle(arena, tree_id, child, generation) };
        }
        status
    })
}

/// Wrap a child in a simple logical-structure role (§14.7–§14.8).
///
/// # Safety
/// `container` and `out_child` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_semantic(
    container: *mut PrismPdfCompositionContainer,
    semantic: PrismPdfCompositionSemantic,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    let semantic = match semantic {
        PrismPdfCompositionSemantic::Paragraph => Semantic::Paragraph,
        PrismPdfCompositionSemantic::List => Semantic::List,
        PrismPdfCompositionSemantic::ListItem => Semantic::ListItem,
        PrismPdfCompositionSemantic::ListLabel => Semantic::ListLabel,
        PrismPdfCompositionSemantic::ListBody => Semantic::ListBody,
        PrismPdfCompositionSemantic::Table => Semantic::Table,
        PrismPdfCompositionSemantic::TableRow => Semantic::TableRow,
        PrismPdfCompositionSemantic::TableHeaderCell => Semantic::TableHeaderCell,
        PrismPdfCompositionSemantic::TableCell => Semantic::TableCell,
    };
    semantic_slot(container, semantic, out_child)
}

/// Wrap a child in a heading role; `level` must be 1–6.
///
/// # Safety
/// `container` and `out_child` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_heading(
    container: *mut PrismPdfCompositionContainer,
    level: u8,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    if !(1..=6).contains(&level) {
        return PrismPdfStatus::Layout;
    }
    semantic_slot(container, Semantic::Heading(level), out_child)
}

/// Wrap a child in an accessible URI-link role.
///
/// # Safety
/// Pointers must be live/writable and strings valid NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_link(
    container: *mut PrismPdfCompositionContainer,
    uri: *const c_char,
    description: *const c_char,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    if uri.is_null() || description.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let (Some(uri), Some(description)) = (unsafe { utf8(uri) }, unsafe { utf8(description) })
    else {
        return PrismPdfStatus::NullArgument;
    };
    semantic_slot(
        container,
        Semantic::Link {
            uri: uri.to_string(),
            description: description.to_string(),
        },
        out_child,
    )
}

/// Wrap a child in a figure role carrying alternate text.
///
/// # Safety
/// `container`/`out_child` must be live/writable and `alt` valid NUL-terminated UTF-8.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_figure(
    container: *mut PrismPdfCompositionContainer,
    alt: *const c_char,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    let Some(alt) = (unsafe { utf8(alt) }) else {
        return PrismPdfStatus::NullArgument;
    };
    semantic_slot(
        container,
        Semantic::Figure {
            alt: alt.to_string(),
        },
        out_child,
    )
}

/// Fill an empty slot with an image using fit, fill, or exact box sizing (§8.9).
///
/// The image is cloned into the composition, so `image` may be released after this call.
///
/// # Safety
/// `container` and `image` must be live handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_image(
    container: *mut PrismPdfCompositionContainer,
    image: *const PrismPdfImageSource,
    sizing: PrismPdfCompositionImageSizing,
    width: f64,
    height: f64,
) -> PrismPdfStatus {
    if container.is_null() || image.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let size = LayoutSize { width, height };
    let sizing = match sizing {
        PrismPdfCompositionImageSizing::Fit => ImageSizing::Fit(size),
        PrismPdfCompositionImageSizing::Fill => ImageSizing::Fill(size),
        PrismPdfCompositionImageSizing::Exact => ImageSizing::Exact(size),
    };
    let image = unsafe { &(*image).0 }.clone();
    guard(|| fill_slot(container, CompositionDraftNode::Image { image, sizing }))
}

/// Wrap a slot in uniform padding and return its empty child.
///
/// # Safety
/// `container` and `out_child` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_padding(
    container: *mut PrismPdfCompositionContainer,
    points: f64,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    decorate_slot(
        container,
        CompositionDraftDecoration::Padding(points),
        out_child,
    )
}

/// Wrap a slot in an exact-width constraint and return its empty child.
///
/// # Safety
/// `container` and `out_child` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_width(
    container: *mut PrismPdfCompositionContainer,
    points: f64,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    decorate_slot(
        container,
        CompositionDraftDecoration::Width(points),
        out_child,
    )
}

/// Wrap a slot in an exact-height constraint and return its empty child.
///
/// # Safety
/// `container` and `out_child` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_height(
    container: *mut PrismPdfCompositionContainer,
    points: f64,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    decorate_slot(
        container,
        CompositionDraftDecoration::Height(points),
        out_child,
    )
}

/// Wrap a slot in an alignment constraint and return its empty child.
///
/// # Safety
/// `container` and `out_child` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_alignment(
    container: *mut PrismPdfCompositionContainer,
    horizontal: PrismPdfCompositionHorizontalAlign,
    vertical: PrismPdfCompositionVerticalAlign,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    let horizontal = match horizontal {
        PrismPdfCompositionHorizontalAlign::Left => LayoutHorizontalAlign::Left,
        PrismPdfCompositionHorizontalAlign::Center => LayoutHorizontalAlign::Center,
        PrismPdfCompositionHorizontalAlign::Right => LayoutHorizontalAlign::Right,
    };
    let vertical = match vertical {
        PrismPdfCompositionVerticalAlign::Top => LayoutVerticalAlign::Top,
        PrismPdfCompositionVerticalAlign::Center => LayoutVerticalAlign::Center,
        PrismPdfCompositionVerticalAlign::Bottom => LayoutVerticalAlign::Bottom,
    };
    decorate_slot(
        container,
        CompositionDraftDecoration::Align(horizontal, vertical),
        out_child,
    )
}

/// Extend a child to consume all offered width and height.
///
/// # Safety
/// `container` and `out_child` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_extend(
    container: *mut PrismPdfCompositionContainer,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    decorate_slot(container, CompositionDraftDecoration::Extend, out_child)
}

pub(crate) fn composition_color(color: PrismPdfCompositionColor) -> LayoutColor {
    LayoutColor::rgb(color.red, color.green, color.blue)
}

/// Paint a border around a child and return its empty slot.
///
/// # Safety
/// `container` and `out_child` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_border(
    container: *mut PrismPdfCompositionContainer,
    width: f64,
    color: PrismPdfCompositionColor,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    decorate_slot(
        container,
        CompositionDraftDecoration::Border(width, composition_color(color)),
        out_child,
    )
}

/// Paint a background behind a child and return its empty slot.
///
/// # Safety
/// `container` and `out_child` must be live/writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_background(
    container: *mut PrismPdfCompositionContainer,
    color: PrismPdfCompositionColor,
    out_child: *mut *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    decorate_slot(
        container,
        CompositionDraftDecoration::Background(composition_color(color)),
        out_child,
    )
}

/// Fill an empty slot with wrapping text using the default Helvetica resource.
///
/// # Safety
/// `container` must be live; `text` must be a valid NUL-terminated UTF-8 string; `style` readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_text(
    container: *mut PrismPdfCompositionContainer,
    text: *const c_char,
    style: *const PrismPdfCompositionTextStyle,
) -> PrismPdfStatus {
    if container.is_null() || text.is_null() || style.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let Some(text) = (unsafe { utf8(text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    let style = unsafe { *style };
    guard(|| {
        fill_slot(
            container,
            CompositionDraftNode::Text {
                text: text.to_string(),
                style,
            },
        )
    })
}

/// Fill an empty slot with an explicit page break.
///
/// # Safety
/// `container` must be live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_container_set_page_break(
    container: *mut PrismPdfCompositionContainer,
) -> PrismPdfStatus {
    guard(|| fill_slot(container, CompositionDraftNode::PageBreak))
}

/// Finalise and build the composition. The handle becomes immutable even when layout fails.
///
/// # Safety
/// `composition` must be live; byte out-pointers writable. Release success bytes with
/// [`prismpdf_bytes_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_composition_build(
    composition: *mut PrismPdfComposition,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if composition.is_null() || out_data.is_null() || out_len.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe {
        *out_data = std::ptr::null_mut();
        *out_len = 0;
    }
    guard(|| {
        let composition = unsafe { &*composition };
        let snapshot = {
            let Ok(mut arena) = composition.0.lock() else {
                return PrismPdfStatus::Internal;
            };
            if !arena.alive || arena.finalised {
                return PrismPdfStatus::InvalidUse;
            }
            arena.finalised = true;
            CompositionArena {
                tree_id: arena.tree_id,
                alive: arena.alive,
                finalised: arena.finalised,
                slots: arena.slots.clone(),
                pages: arena.pages.clone(),
                lang: arena.lang.clone(),
            }
        };
        match build_draft(&snapshot) {
            Ok(bytes) => emit_bytes(bytes, out_data, out_len),
            Err(error) => composition_status(error),
        }
    })
}
