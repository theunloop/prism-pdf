use super::*;
use pdf_document::Document;

fn short_style(content_height: f64) -> PageStyle {
    PageStyle {
        size: [200.0, content_height + 20.0],
        margins: [10.0; 4],
    }
}

fn two_lines(style: PageStyle) -> Result<ComposedDocument, ComposeError> {
    Composition::new()
        .page(style, |page| {
            page.content().column(|column| {
                column
                    .item()
                    .text("first\nsecond", TextStyle::new().size(12.0).leading(14.0));
            });
        })
        .build()
}

#[test]
fn exact_fit_is_one_round_trippable_page() {
    let output = two_lines(short_style(28.0)).unwrap();
    let document = Document::open(output.pdf().to_vec()).unwrap();
    assert_eq!(document.page_count().unwrap(), 1);
    let text_events = output
        .trace()
        .events()
        .iter()
        .filter(|event| event.kind == "Text")
        .collect::<Vec<_>>();
    assert_eq!(text_events.len(), 1);
    assert_eq!(text_events[0].bounds.size.height, 28.0);
    assert_eq!(text_events[0].text.as_deref(), Some("first\nsecond"));
}

#[test]
fn one_point_overflow_paginates_with_stable_origins() {
    let output = two_lines(short_style(27.0)).unwrap();
    let document = Document::open(output.pdf().to_vec()).unwrap();
    assert_eq!(document.page_count().unwrap(), 2);
    let text_events = output
        .trace()
        .events()
        .iter()
        .filter(|event| event.kind == "Text")
        .collect::<Vec<_>>();
    assert_eq!(text_events.len(), 2);
    assert_eq!(text_events[0].page, 0);
    assert_eq!(text_events[1].page, 1);
    assert_eq!(text_events[0].bounds.origin, text_events[1].bounds.origin);
    assert_eq!(text_events[0].text.as_deref(), Some("first"));
    assert_eq!(text_events[1].text.as_deref(), Some("second"));
}

#[test]
fn over_tall_text_fails_instead_of_retrying() {
    let error = two_lines(short_style(13.0)).unwrap_err();
    assert_eq!(error, ComposeError::OverTallElement);
}

#[test]
fn empty_composition_and_empty_page_each_emit_one_page() {
    for composition in [
        Composition::new(),
        Composition::new().page(PageStyle::default(), |_| {}),
    ] {
        let output = composition.build().unwrap();
        assert!(output.trace().events().is_empty());
        assert_eq!(
            Document::open(output.into_pdf())
                .unwrap()
                .page_count()
                .unwrap(),
            1
        );
    }
}

#[test]
fn repeated_measurement_is_deterministic_and_reset_rewinds_consumption() {
    let fonts = BTreeMap::from([("F1".to_string(), FontSlot::Standard(StdFont::Helvetica))]);
    let metrics = Metrics::new(&fonts);
    let available = Size::new(100.0, 14.0);
    let mut text = TextNode::new("first\nsecond", TextStyle::new().size(12.0).leading(14.0));
    let first = text
        .measure(available, &metrics, HeightMode::Offered)
        .unwrap();
    let repeated = text
        .measure(available, &metrics, HeightMode::Offered)
        .unwrap();
    assert_eq!(first, repeated);

    let mut content = Content::new();
    let mut images = Vec::new();
    let mut mcid_next = 0;
    let mut marked_depth = 0;
    let mut annotations = Vec::new();
    let mut trace = GeometryTrace::default();
    let mut context = DrawCtx {
        content: &mut content,
        metrics: &metrics,
        trace: &mut trace,
        page: 0,
        page_height: 100.0,
        origin: Point::default(),
        images: &mut images,
        tagged: false,
        mcid_next: &mut mcid_next,
        annotations: &mut annotations,
        marked_depth: &mut marked_depth,
    };
    let Plan::Partial(size) = repeated else {
        panic!("one of two lines must be partial");
    };
    text.draw(&mut context, size).unwrap();
    assert_eq!(trace.events()[0].text.as_deref(), Some("first"));

    text.reset();
    let reset_plan = text
        .measure(available, &metrics, HeightMode::Offered)
        .unwrap();
    assert_eq!(reset_plan, first);
}

#[test]
fn draw_rejects_a_size_other_than_the_measured_size() {
    let fonts = BTreeMap::from([("F1".to_string(), FontSlot::Standard(StdFont::Helvetica))]);
    let metrics = Metrics::new(&fonts);
    let mut text = TextNode::new("line", TextStyle::new());
    let _ = text
        .measure(Size::new(100.0, 100.0), &metrics, HeightMode::Offered)
        .unwrap();
    let mut content = Content::new();
    let mut images = Vec::new();
    let mut mcid_next = 0;
    let mut marked_depth = 0;
    let mut annotations = Vec::new();
    let mut trace = GeometryTrace::default();
    let mut context = DrawCtx {
        content: &mut content,
        metrics: &metrics,
        trace: &mut trace,
        page: 0,
        page_height: 100.0,
        origin: Point::default(),
        images: &mut images,
        tagged: false,
        mcid_next: &mut mcid_next,
        annotations: &mut annotations,
        marked_depth: &mut marked_depth,
    };
    assert_eq!(
        text.draw(&mut context, Size::new(99.0, 14.0)),
        Err(ComposeError::MeasurementMismatch)
    );
}

#[test]
fn zero_progress_is_rejected() {
    assert_eq!(
        checked_size(Plan::Partial(Size::new(10.0, 0.0)), true),
        Err(ComposeError::NoProgress)
    );
}

#[test]
fn missing_font_and_invalid_geometry_are_actionable() {
    let missing = Composition::new()
        .page(PageStyle::default(), |page| {
            page.content()
                .text("hello", TextStyle::new().font("Missing"));
        })
        .build()
        .unwrap_err();
    assert_eq!(missing, ComposeError::MissingFont("Missing".to_string()));

    let invalid = Composition::new()
        .page(short_style(f64::NAN), |_| {})
        .build()
        .unwrap_err();
    assert_eq!(invalid, ComposeError::InvalidGeometry);
}

#[test]
fn column_spacing_is_reflected_in_geometry() {
    let output = Composition::new()
        .page(short_style(60.0), |page| {
            page.content().column(|column| {
                column.spacing(5.0);
                column.item().text("one", TextStyle::new());
                column.item().text("two", TextStyle::new());
            });
        })
        .build()
        .unwrap();
    let text = output
        .trace()
        .events()
        .iter()
        .filter(|event| event.kind == "Text")
        .collect::<Vec<_>>();
    assert_eq!(text.len(), 2);
    assert_eq!(
        text[1].bounds.origin.y,
        text[0].bounds.origin.y + text[0].bounds.size.height + 5.0
    );
}

#[test]
fn row_allocates_fixed_relative_and_automatic_widths() {
    let output = Composition::new()
        .standard_font("F2", StdFont::Courier)
        .page(short_style(40.0), |page| {
            page.content().row(|row| {
                row.fixed(40.0).text("fixed", TextStyle::new());
                row.relative(1.0).text("relative", TextStyle::new());
                row.auto()
                    .text("xx", TextStyle::new().font("F2").size(10.0).leading(12.0));
            });
        })
        .build()
        .unwrap();
    let text = output
        .trace()
        .events()
        .iter()
        .filter(|event| event.kind == "Text")
        .collect::<Vec<_>>();
    assert_eq!(text.len(), 3);
    assert_eq!(text[0].bounds.origin.x, 10.0);
    assert_eq!(text[1].bounds.origin.x, 50.0);
    // Courier "xx" at 10 pt is 12 pt wide, leaving 128 pt to the relative child.
    assert_eq!(text[2].bounds.origin.x, 178.0);
    let row = output
        .trace()
        .events()
        .iter()
        .find(|event| event.kind == "Row")
        .unwrap();
    assert_eq!(row.bounds.size.width, 180.0);
}

#[test]
fn indivisible_row_wraps_to_the_next_page() {
    let output = Composition::new()
        .page(short_style(41.0), |page| {
            page.content().column(|column| {
                column.item().text("before", TextStyle::new().leading(14.0));
                column.item().row(|row| {
                    row.relative(1.0)
                        .text("line one\nline two", TextStyle::new().leading(14.0));
                });
            });
        })
        .build()
        .unwrap();
    assert_eq!(
        Document::open(output.pdf().to_vec())
            .unwrap()
            .page_count()
            .unwrap(),
        2
    );
    let row = output
        .trace()
        .events()
        .iter()
        .find(|event| event.kind == "Row")
        .unwrap();
    assert_eq!(row.page, 1);
    assert_eq!(row.bounds.origin.y, 10.0);
}

#[test]
fn over_tall_or_invalid_row_fails_cleanly() {
    let over_tall = Composition::new()
        .page(short_style(27.0), |page| {
            page.content().row(|row| {
                row.relative(1.0)
                    .text("line one\nline two", TextStyle::new().leading(14.0));
            });
        })
        .build()
        .unwrap_err();
    assert_eq!(over_tall, ComposeError::OverTallElement);

    for invalid in [
        Composition::new().page(short_style(40.0), |page| {
            page.content().row(|row| {
                row.fixed(-1.0).text("bad", TextStyle::new());
            });
        }),
        Composition::new().page(short_style(40.0), |page| {
            page.content().row(|row| {
                row.relative(0.0).text("bad", TextStyle::new());
            });
        }),
        Composition::new().page(short_style(40.0), |page| {
            page.content().row(|row| {
                row.fixed(181.0).text("bad", TextStyle::new());
                row.auto().text("more", TextStyle::new());
            });
        }),
    ] {
        assert_eq!(invalid.build().unwrap_err(), ComposeError::InvalidGeometry);
    }
}

/// The reporter's own repro, from a .NET integration: a three-row, one-column table whose cells
/// ask to centre their text vertically. Every row used to become a full page, because the height a
/// row offers a cell during measurement is the rest of the page — a pagination budget, not a box —
/// and vertical alignment is implemented by filling the box. A forty-row table was one call away
/// from a forty-page document, and nothing at the call site said so.
#[test]
fn a_vertical_alignment_in_a_table_cell_fills_the_row_not_the_page() {
    fn pages(cell: fn(&mut Container<'_>, &str)) -> usize {
        let output = Composition::new()
            .page(short_style(400.0), |page| {
                page.content().table(|table| {
                    table.relative_column(1.0);
                    for label in ["ROW1", "ROW2", "ROW3"] {
                        table.row(|row| cell(&mut row.cell(), label));
                    }
                });
            })
            .build()
            .unwrap();
        Document::open(output.into_pdf())
            .unwrap()
            .page_count()
            .unwrap()
    }

    // The baseline every other case is judged against.
    assert_eq!(
        pages(|cell, label| cell.text(label, TextStyle::new().size(12.0).leading(14.0))),
        1
    );
    for aligned in [VerticalAlign::Center, VerticalAlign::Bottom] {
        assert_eq!(
            pages(match aligned {
                VerticalAlign::Center => |cell: &mut Container<'_>, label: &str| {
                    cell.align(HorizontalAlign::Left, VerticalAlign::Center, |inner| {
                        inner.text(label, TextStyle::new().size(12.0).leading(14.0));
                    });
                },
                _ => |cell: &mut Container<'_>, label: &str| {
                    cell.align(HorizontalAlign::Left, VerticalAlign::Bottom, |inner| {
                        inner.text(label, TextStyle::new().size(12.0).leading(14.0));
                    });
                },
            }),
            1,
            "{aligned:?} alignment stays inside its row"
        );
    }
    // An explicit extend is the same claim by a different name, and now means the same box.
    assert_eq!(
        pages(|cell, label| {
            cell.extend(|inner| {
                inner.text(label, TextStyle::new().size(12.0).leading(14.0));
            });
        }),
        1
    );
}

/// The counterpart: outside a table nothing changes. A page column offers the room left on the
/// page, and that genuinely is the box a caller means when they centre a block there.
#[test]
fn a_vertical_alignment_on_a_page_still_uses_the_page() {
    let output = Composition::new()
        .page(short_style(200.0), |page| {
            page.content()
                .align(HorizontalAlign::Center, VerticalAlign::Center, |aligned| {
                    aligned.text("centred", TextStyle::new().size(12.0).leading(14.0));
                });
        })
        .build()
        .unwrap();
    let text = output
        .trace()
        .events()
        .iter()
        .find(|event| event.kind == "Text")
        .unwrap();
    // Content runs from y=10 for 200 points; a 14-point line centred in it starts at 103.
    assert_eq!(text.bounds.origin.y, 103.0);
}

/// A row takes its height from the cell that needs the most, and the shorter cells are told what
/// that height is — which is what lets one of them align its content against the others.
#[test]
fn a_row_settles_on_the_height_its_tallest_cell_needs() {
    let output = Composition::new()
        .page(short_style(400.0), |page| {
            page.content().table(|table| {
                table.relative_column(1.0);
                table.relative_column(1.0);
                table.row(|row| {
                    row.cell()
                        .text("one\ntwo\nthree", TextStyle::new().size(12.0).leading(14.0));
                    row.cell()
                        .align(HorizontalAlign::Left, VerticalAlign::Bottom, |inner| {
                            inner.text("last", TextStyle::new().size(12.0).leading(14.0));
                        });
                });
            });
        })
        .build()
        .unwrap();
    let texts: Vec<_> = output
        .trace()
        .events()
        .iter()
        .filter(|event| event.kind == "Text")
        .collect();
    assert_eq!(texts.len(), 2);
    // Three lines at 14 points make the row 42 tall; the bottom-aligned cell sits on its last
    // line rather than on the foot of the page.
    assert_eq!(texts[0].bounds.size.height, 42.0);
    assert_eq!(texts[1].bounds.origin.y, texts[0].bounds.origin.y + 28.0);
}

#[test]
fn decorators_compose_constraints_alignment_and_paint() {
    let output = Composition::new()
        .page(short_style(80.0), |page| {
            page.content()
                .background(Color::rgb(0.9, 0.8, 0.7), |background| {
                    background.border(2.0, Color::rgb(0.1, 0.2, 0.3), |border| {
                        border.padding(5.0, |padding| {
                            padding.height(40.0, |height| {
                                height.align(
                                    HorizontalAlign::Right,
                                    VerticalAlign::Bottom,
                                    |aligned| {
                                        aligned.width(60.0, |width| {
                                            width.text("placed", TextStyle::new().leading(14.0));
                                        });
                                    },
                                );
                            });
                        });
                    });
                });
        })
        .build()
        .unwrap();
    let text = output
        .trace()
        .events()
        .iter()
        .find(|event| event.kind == "Text")
        .unwrap();
    assert_eq!(text.bounds.origin, Point { x: 125.0, y: 41.0 });
    assert_eq!(text.bounds.size.height, 14.0);
    assert_eq!(
        output
            .trace()
            .events()
            .iter()
            .filter(|event| event.kind == "Decorated")
            .count(),
        6
    );
    assert_eq!(
        Document::open(output.into_pdf())
            .unwrap()
            .page_count()
            .unwrap(),
        1
    );
}

#[test]
fn decorated_partial_content_keeps_padding_on_each_page() {
    let output = Composition::new()
        .page(short_style(38.0), |page| {
            page.content().padding(5.0, |padding| {
                padding.text("first\nsecond\nthird", TextStyle::new().leading(14.0));
            });
        })
        .build()
        .unwrap();
    let text = output
        .trace()
        .events()
        .iter()
        .filter(|event| event.kind == "Text")
        .collect::<Vec<_>>();
    assert_eq!(text.len(), 2);
    assert_eq!(text[0].bounds.origin, Point { x: 15.0, y: 15.0 });
    assert_eq!(text[1].bounds.origin, Point { x: 15.0, y: 15.0 });
    assert_eq!(text[0].text.as_deref(), Some("first\nsecond"));
    assert_eq!(text[1].text.as_deref(), Some("third"));
}

#[test]
fn invalid_decorator_geometry_is_rejected() {
    let cases = [
        Composition::new().page(short_style(40.0), |page| {
            page.content().padding(-1.0, |child| {
                child.text("bad", TextStyle::new());
            });
        }),
        Composition::new().page(short_style(40.0), |page| {
            page.content().width(181.0, |child| {
                child.text("bad", TextStyle::new());
            });
        }),
        Composition::new().page(short_style(40.0), |page| {
            page.content()
                .background(Color::rgb(1.1, 0.0, 0.0), |child| {
                    child.text("bad", TextStyle::new());
                });
        }),
    ];
    for case in cases {
        assert_eq!(case.build().unwrap_err(), ComposeError::InvalidGeometry);
    }
}

#[test]
fn explicit_page_break_starts_following_content_at_the_top() {
    let output = Composition::new()
        .page(short_style(80.0), |page| {
            page.content().column(|column| {
                column.item().text("before", TextStyle::new());
                column.item().page_break();
                column.item().text("after", TextStyle::new());
            });
        })
        .build()
        .unwrap();
    let text = output
        .trace()
        .events()
        .iter()
        .filter(|event| event.kind == "Text")
        .collect::<Vec<_>>();
    assert_eq!(text.len(), 2);
    assert_eq!(text[0].page, 0);
    assert_eq!(text[1].page, 1);
    assert_eq!(text[1].bounds.origin.y, 10.0);
}

#[test]
fn header_footer_and_total_page_placeholders_repeat() {
    let output = Composition::new()
        .page(short_style(48.0), |page| {
            page.header()
                .text("Invoice", TextStyle::new().size(8.0).leading(10.0));
            page.footer().text(
                "Page {page} of {pages}",
                TextStyle::new().size(8.0).leading(10.0),
            );
            page.content().text(
                "first\nsecond\nthird\nfourth",
                TextStyle::new().leading(14.0),
            );
        })
        .build()
        .unwrap();
    let document = Document::open(output.pdf().to_vec()).unwrap();
    assert_eq!(document.page_count().unwrap(), 2);
    let text = output
        .trace()
        .events()
        .iter()
        .filter(|event| event.kind == "Text")
        .collect::<Vec<_>>();
    assert_eq!(text.len(), 6);
    for page in 0..2 {
        let page_text = text
            .iter()
            .filter(|event| event.page == page)
            .collect::<Vec<_>>();
        assert_eq!(page_text[0].text.as_deref(), Some("Invoice"));
        assert_eq!(page_text[0].bounds.origin.y, 10.0);
        assert_eq!(page_text[1].bounds.origin.y, 20.0);
        assert_eq!(page_text[2].bounds.origin.y, 48.0);
        assert_eq!(page_text[2].text, Some(format!("Page {} of 2", page + 1)));
    }
}

#[test]
fn repeating_regions_cannot_consume_the_entire_page() {
    let error = Composition::new()
        .page(short_style(20.0), |page| {
            page.header().text("header", TextStyle::new().leading(10.0));
            page.footer().text("footer", TextStyle::new().leading(10.0));
            page.content().text("body", TextStyle::new());
        })
        .build()
        .unwrap_err();
    assert_eq!(error, ComposeError::OverTallElement);
}

#[test]
fn table_cells_hold_trees_and_repeat_the_header() {
    let output = Composition::new()
        .standard_font("F2", StdFont::Courier)
        .page(short_style(50.0), |page| {
            page.content().table(|table| {
                table.automatic_column();
                table.relative_column(1.0);
                table.header(|row| {
                    row.cell().padding(1.0, |cell| {
                        cell.text("Code", TextStyle::new().font("F2").size(10.0).leading(12.0));
                    });
                    row.cell()
                        .text("Description", TextStyle::new().leading(14.0));
                });
                for (code, description) in [("A", "one"), ("BB", "two"), ("CCC", "three")] {
                    table.row(|row| {
                        row.cell().padding(1.0, |cell| {
                            cell.text(code, TextStyle::new().font("F2").size(10.0).leading(12.0));
                        });
                        row.cell().text(description, TextStyle::new().leading(14.0));
                    });
                }
            });
        })
        .build()
        .unwrap();
    assert_eq!(
        Document::open(output.pdf().to_vec())
            .unwrap()
            .page_count()
            .unwrap(),
        2
    );
    let headers = output
        .trace()
        .events()
        .iter()
        .filter(|event| event.text.as_deref() == Some("Description"))
        .collect::<Vec<_>>();
    assert_eq!(headers.len(), 2);
    assert_eq!(headers[0].bounds.origin, Point { x: 36.0, y: 10.0 });
    assert_eq!(headers[1].bounds.origin, Point { x: 36.0, y: 10.0 });
    assert_eq!(
        output
            .trace()
            .events()
            .iter()
            .filter(|event| event.kind == "Table")
            .count(),
        2
    );
}

#[test]
fn table_rejects_invalid_shapes_and_over_tall_rows() {
    let invalid = Composition::new()
        .page(short_style(50.0), |page| {
            page.content().table(|table| {
                table.relative_column(1.0);
                table.row(|row| {
                    row.cell().text("one", TextStyle::new());
                    row.cell().text("extra", TextStyle::new());
                });
            });
        })
        .build()
        .unwrap_err();
    assert_eq!(invalid, ComposeError::InvalidGeometry);

    let over_tall = Composition::new()
        .page(short_style(20.0), |page| {
            page.content().table(|table| {
                table.relative_column(1.0);
                table.row(|row| {
                    row.cell().text("one\ntwo", TextStyle::new().leading(14.0));
                });
            });
        })
        .build()
        .unwrap_err();
    assert_eq!(over_tall, ComposeError::OverTallElement);
}

#[test]
fn images_support_fit_fill_and_exact_sizing() {
    let image = crate::Image::from_rgb(2, 1, vec![255, 0, 0, 0, 255, 0]).unwrap();
    let output = Composition::new()
        .page(short_style(170.0), |page| {
            page.content().column(|column| {
                column
                    .item()
                    .image(&image, ImageSizing::Fit(Size::new(80.0, 80.0)));
                column
                    .item()
                    .image(&image, ImageSizing::Fill(Size::new(80.0, 60.0)));
                column
                    .item()
                    .image(&image, ImageSizing::Exact(Size::new(40.0, 20.0)));
            });
        })
        .build()
        .unwrap();
    let images = output
        .trace()
        .events()
        .iter()
        .filter(|event| event.kind == "Image")
        .collect::<Vec<_>>();
    assert_eq!(images.len(), 3);
    assert_eq!(images[0].bounds.size, Size::new(80.0, 40.0));
    assert_eq!(images[1].bounds.size, Size::new(80.0, 60.0));
    assert_eq!(images[2].bounds.size, Size::new(40.0, 20.0));
    let document = Document::open(output.into_pdf()).unwrap();
    let page = &document.pages().unwrap()[0];
    let content = document.page_content_bytes(page).unwrap();
    let operators = String::from_utf8(content).unwrap();
    assert_eq!(operators.matches(" Do\n").count(), 3);
    assert!(operators.contains("W\nn\n"));
}

#[test]
fn image_wraps_or_rejects_invalid_geometry() {
    let image = crate::Image::from_gray(1, 1, vec![0]).unwrap();
    let output = Composition::new()
        .page(short_style(40.0), |page| {
            page.content().column(|column| {
                column.item().text("before", TextStyle::new().leading(14.0));
                column
                    .item()
                    .image(&image, ImageSizing::Exact(Size::new(20.0, 30.0)));
            });
        })
        .build()
        .unwrap();
    let event = output
        .trace()
        .events()
        .iter()
        .find(|event| event.kind == "Image")
        .unwrap();
    assert_eq!(event.page, 1);
    assert_eq!(event.bounds.origin.y, 10.0);

    let invalid = Composition::new()
        .page(short_style(40.0), |page| {
            page.content()
                .image(&image, ImageSizing::Fit(Size::new(0.0, 20.0)));
        })
        .build()
        .unwrap_err();
    assert_eq!(invalid, ComposeError::InvalidGeometry);
}

#[test]
fn tagged_composition_emits_nested_semantics_links_and_figures() {
    let image = crate::Image::from_rgb(1, 1, vec![255, 0, 0]).unwrap();
    let output = Composition::new()
        .tagged("en-US")
        .page(short_style(300.0), |page| {
            page.header().semantic(Semantic::Paragraph, |header| {
                header.text("artifact header", TextStyle::new());
            });
            page.content().column(|column| {
                column.item().semantic(Semantic::Heading(1), |heading| {
                    heading.text("Invoice", TextStyle::new());
                });
                column.item().semantic(Semantic::Paragraph, |paragraph| {
                    paragraph.text("Introduction", TextStyle::new());
                });
                column.item().semantic(Semantic::List, |list| {
                    list.column(|items| {
                        items.item().semantic(Semantic::ListItem, |item| {
                            item.row(|row| {
                                row.fixed(15.0).semantic(Semantic::ListLabel, |label| {
                                    label.text("•", TextStyle::new());
                                });
                                row.relative(1.0).semantic(Semantic::ListBody, |body| {
                                    body.text("Item", TextStyle::new());
                                });
                            });
                        });
                    });
                });
                column.item().semantic(Semantic::Table, |table_semantic| {
                    table_semantic.table(|table| {
                        table.relative_column(1.0);
                        table.header(|row| {
                            row.cell().semantic(Semantic::TableHeaderCell, |cell| {
                                cell.text("Name", TextStyle::new());
                            });
                        });
                        table.row(|row| {
                            row.cell().semantic(Semantic::TableCell, |cell| {
                                cell.text("Widget", TextStyle::new());
                            });
                        });
                    });
                });
                column.item().semantic(
                    Semantic::Link {
                        uri: "https://example.com".to_string(),
                        description: "Example website".to_string(),
                    },
                    |link| link.text("Visit", TextStyle::new()),
                );
                column.item().semantic(
                    Semantic::Figure {
                        alt: "A red square".to_string(),
                    },
                    |figure| {
                        figure.image(&image, ImageSizing::Exact(Size::new(20.0, 20.0)));
                    },
                );
            });
        })
        .build()
        .unwrap();
    let pdf = String::from_utf8_lossy(output.pdf());
    for tag in [
        "/S /H1",
        "/S /P",
        "/S /L",
        "/S /LI",
        "/S /Lbl",
        "/S /LBody",
        "/S /Table",
        "/S /TR",
        "/S /TH",
        "/S /TD",
        "/S /Link",
        "/S /Figure",
    ] {
        assert!(pdf.contains(tag), "missing {tag}");
    }
    assert!(pdf.contains("/Scope /Column"));
    assert!(pdf.contains("/Alt (A red square)"));
    assert!(pdf.contains("/Subtype /Link"));
    assert!(pdf.contains("/OBJR"));
    assert!(pdf.contains("/Lang (en-US)"));
    assert!(pdf.contains("/Artifact BMC"));
    assert_eq!(pdf.matches("/S /P").count(), 1, "header is not structure");
    assert_eq!(
        Document::open(output.into_pdf())
            .unwrap()
            .page_count()
            .unwrap(),
        1
    );
}

#[test]
fn semantic_validation_and_opt_in_are_explicit() {
    let invalid = Composition::new()
        .tagged("en")
        .page(short_style(40.0), |page| {
            page.content().semantic(Semantic::Heading(7), |heading| {
                heading.text("bad", TextStyle::new());
            });
        })
        .build()
        .unwrap_err();
    assert_eq!(invalid, ComposeError::InvalidGeometry);

    let plain = Composition::new()
        .page(short_style(40.0), |page| {
            page.content().semantic(Semantic::Paragraph, |paragraph| {
                paragraph.text("plain", TextStyle::new());
            });
        })
        .build()
        .unwrap();
    let pdf = String::from_utf8_lossy(plain.pdf());
    assert!(!pdf.contains("/StructTreeRoot"));
    assert!(!pdf.contains("BDC"));
}

#[test]
fn semantic_content_spans_pages_with_per_page_mcids() {
    let output = Composition::new()
        .tagged("en")
        .page(short_style(20.0), |page| {
            page.content().semantic(Semantic::Paragraph, |paragraph| {
                paragraph.text("first\nsecond", TextStyle::new().leading(14.0));
            });
        })
        .build()
        .unwrap();
    let document = Document::open(output.pdf().to_vec()).unwrap();
    assert_eq!(document.page_count().unwrap(), 2);
    for page in document.pages().unwrap() {
        let content = String::from_utf8(document.page_content_bytes(&page).unwrap()).unwrap();
        assert!(content.contains("/P <</MCID 0>> BDC"));
    }
    let pdf = String::from_utf8_lossy(output.pdf());
    assert_eq!(pdf.matches("/Type /MCR").count(), 2);
    assert_eq!(pdf.matches("/S /P").count(), 1);
}

/// A decorated box that does not fit the room left on the page belongs on the next one. It used to
/// fail the whole document with `InvalidGeometry` instead, on a tree whose every size, margin and
/// leading was finite and positive — and the band was only as wide as the box's own padding, so
/// which documents hit it looked arbitrary from outside.
#[test]
fn padded_child_wraps_when_the_page_has_less_room_than_its_padding() {
    let tree = |height: f64| {
        Composition::new()
            .page(short_style(height), |page| {
                page.content().column(|column| {
                    column
                        .item()
                        .text("first\nsecond", TextStyle::new().leading(14.0));
                    column.item().padding(4.0, |cell| {
                        cell.text("after", TextStyle::new().leading(14.0));
                    });
                });
            })
            .build()
    };

    // The padded box is 22pt tall and fits a fresh page either way. Only the room left under the
    // first child differs: 10pt at a content height of 38, 2pt at 30 — and 2pt is under the box's
    // own 8pt of vertical padding, which is what used to be fatal.
    for content_height in [38.0, 30.0] {
        let output = tree(content_height).unwrap();
        let document = Document::open(output.pdf().to_vec()).unwrap();
        assert_eq!(document.page_count().unwrap(), 2, "at {content_height}");
        let text = output
            .trace()
            .events()
            .iter()
            .filter(|event| event.kind == "Text")
            .map(|event| event.text.as_deref().unwrap_or_default().to_string())
            .collect::<Vec<_>>();
        assert_eq!(text, ["first\nsecond", "after"], "at {content_height}");
    }
}

/// The same for a fixed height, which had no working window at all: the box fits a fresh page with
/// room to spare, so it is a page break and not an over-tall element.
#[test]
fn fixed_height_child_wraps_to_the_next_page() {
    let output = Composition::new()
        .page(short_style(38.0), |page| {
            page.content().column(|column| {
                column
                    .item()
                    .text("first\nsecond", TextStyle::new().leading(14.0));
                column.item().height(20.0, |box_| {
                    box_.text("after", TextStyle::new().leading(14.0));
                });
            });
        })
        .build()
        .unwrap();
    let document = Document::open(output.pdf().to_vec()).unwrap();
    assert_eq!(document.page_count().unwrap(), 2);
}

/// The other half of the same branch: a box no page can hold is still an error, and now reports the
/// variant that says so rather than `InvalidGeometry`.
#[test]
fn a_box_no_page_can_hold_is_still_over_tall() {
    let cases = [
        Composition::new().page(short_style(40.0), |page| {
            page.content().height(500.0, |child| {
                child.text("tall", TextStyle::new());
            });
        }),
        Composition::new().page(short_style(40.0), |page| {
            page.content().padding(300.0, |child| {
                child.text("padded", TextStyle::new());
            });
        }),
        // A running header is measured against the whole content area and never paginates.
        Composition::new().page(short_style(40.0), |page| {
            page.header().height(500.0, |child| {
                child.text("header", TextStyle::new());
            });
            page.content().text("body", TextStyle::new());
        }),
    ];
    for case in cases {
        assert_eq!(case.build().unwrap_err(), ComposeError::OverTallElement);
    }
}

/// A decoration that is not a decoration at all stays `InvalidGeometry`, however much room the page
/// has left. This pins the ordering: the finite/non-negative validation has to run before the
/// height comparison, because every comparison against a NaN is false and would otherwise read as
/// "does not fit" — turning bad input into a silent page break.
#[test]
fn a_non_finite_decoration_is_still_invalid_geometry() {
    for value in [f64::NAN, f64::INFINITY, -1.0] {
        let padding = Composition::new().page(short_style(40.0), |page| {
            page.content().padding(value, |child| {
                child.text("bad", TextStyle::new());
            });
        });
        assert_eq!(
            padding.build().unwrap_err(),
            ComposeError::InvalidGeometry,
            "padding {value}"
        );
        let height = Composition::new().page(short_style(40.0), |page| {
            page.content().height(value, |child| {
                child.text("bad", TextStyle::new());
            });
        });
        assert_eq!(
            height.build().unwrap_err(),
            ComposeError::InvalidGeometry,
            "height {value}"
        );
    }
}

/// Settling a row's height is a table's business. A row a caller placed themselves is offered the
/// box they meant — the rest of the page — and a cell that fills still fills it.
#[test]
fn a_row_outside_a_table_still_fills_the_page() {
    let output = Composition::new()
        .page(short_style(400.0), |page| {
            page.content().row(|row| {
                row.relative(1.0)
                    .extend(|cell| cell.text("left", TextStyle::new().size(12.0).leading(14.0)));
                row.relative(1.0)
                    .text("right", TextStyle::new().size(12.0).leading(14.0));
            });
        })
        .build()
        .unwrap();
    let row = output
        .trace()
        .events()
        .iter()
        .find(|event| event.kind == "Row")
        .unwrap();
    assert_eq!(row.bounds.size.height, 400.0);
}
