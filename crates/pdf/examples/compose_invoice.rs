//! Generate the M25 composition acceptance invoice for manual inspection.

use std::error::Error;
use std::path::PathBuf;

use prismpdf::{
    Color, Composition, HorizontalAlign, PageStyle, Semantic, TextStyle, VerticalAlign,
};

fn main() -> Result<(), Box<dyn Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("compose-invoice.pdf"));

    let small = TextStyle::new().size(9.0).leading(12.0);
    let body = TextStyle::new().size(10.0).leading(14.0);
    let heading = TextStyle::new().size(20.0).leading(24.0);
    let rule = Color::rgb(0.78, 0.82, 0.87);
    let navy = Color::rgb(0.08, 0.18, 0.32);
    let pale = Color::rgb(0.93, 0.96, 0.99);

    let document = Composition::new()
        .tagged("en-US")
        .page(PageStyle::a4(42.0), |page| {
            page.header().background(pale, |band| {
                band.border(0.8, navy, |border| {
                    border.padding(10.0, |padding| {
                        padding.row(|row| {
                        row.relative(1.0).semantic(Semantic::Paragraph, |cell| {
                            cell.text("Prism PDF Studio", TextStyle::new().size(11.0).leading(14.0));
                        });
                        row.relative(1.0).align(
                            HorizontalAlign::Right,
                            VerticalAlign::Top,
                            |cell| {
                                cell.semantic(Semantic::Paragraph, |text| {
                                    text.text(
                                        "Invoice 2026-0042",
                                        TextStyle::new().size(10.0).leading(14.0),
                                    );
                                });
                            },
                        );
                        });
                    });
                });
            });
            page.footer().align(
                HorizontalAlign::Center,
                VerticalAlign::Top,
                |footer| {
                    footer.semantic(Semantic::Paragraph, |text| {
                        text.text("Page {page} of {pages}", small.clone());
                    });
                },
            );

            page.content().column(|column| {
                column.spacing(14.0);
                column.item().row(|row| {
                    row.relative(1.0).semantic(Semantic::Heading(1), |title| {
                        title.text("INVOICE", heading.clone());
                    });
                    row.relative(1.0).align(
                        HorizontalAlign::Right,
                        VerticalAlign::Top,
                        |details| {
                            details.semantic(Semantic::Paragraph, |paragraph| {
                                paragraph.text(
                                    "No. 2026-0042\nIssued: 24 Aug 2026\nDue: 23 Sep 2026",
                                    small.clone(),
                                );
                            });
                        },
                    );
                });

                column.item().row(|row| {
                    for address in [
                        "BILL TO\nAcme Corporation\n1 Main Street\nLondon EC1A 1AA",
                        "SHIP TO\nAcme Warehouse\n9 Dock Road\nLiverpool L1 8JQ",
                    ] {
                        row.relative(1.0).background(pale, |box_| {
                            box_.border(0.6, rule, |border| {
                                border.align(
                                    HorizontalAlign::Left,
                                    VerticalAlign::Top,
                                    |full_width| {
                                        full_width.padding(8.0, |padding| {
                                            padding.semantic(
                                                Semantic::Paragraph,
                                                |paragraph| {
                                                    paragraph.text(address, body.clone());
                                                },
                                            );
                                        });
                                    },
                                );
                            });
                        });
                    }
                });

                column.item().semantic(Semantic::Table, |table_semantic| {
                    table_semantic.table(|table| {
                        table.relative_column(0.8);
                        table.relative_column(4.2);
                        table.relative_column(1.0);
                        table.relative_column(1.3);
                        table.header(|row| {
                            for label in ["ITEM", "DESCRIPTION", "QTY", "AMOUNT"] {
                                row.cell().background(pale, |background| {
                                    background.border(0.6, navy, |border| {
                                        border.align(
                                            HorizontalAlign::Left,
                                            VerticalAlign::Top,
                                            |full_width| {
                                                full_width.padding(5.0, |padding| {
                                                    padding.semantic(
                                                        Semantic::TableHeaderCell,
                                                        |cell| {
                                                            cell.text(label, small.clone());
                                                        },
                                                    );
                                                });
                                            },
                                        );
                                    });
                                });
                            }
                        });
                        for index in 1..=32 {
                            let description = if index % 5 == 0 {
                                "Custom integration and verification support with detailed delivery notes"
                            } else {
                                "Document processing service"
                            };
                            let values = [
                                format!("{index:02}"),
                                description.to_string(),
                                format!("{}", index % 4 + 1),
                                format!("{:>7.2}", 37.5 * f64::from(index)),
                            ];
                            table.row(|row| {
                                for value in values {
                                    row.cell().border(0.35, rule, |border| {
                                        border.align(
                                            HorizontalAlign::Left,
                                            VerticalAlign::Top,
                                            |full_width| {
                                                full_width.padding(4.0, |padding| {
                                                    padding.semantic(
                                                        Semantic::TableCell,
                                                        |cell| {
                                                            cell.text(&value, small.clone());
                                                        },
                                                    );
                                                });
                                            },
                                        );
                                    });
                                }
                            });
                        }
                    });
                });

                column.item().row(|row| {
                    row.relative(5.0).semantic(Semantic::Paragraph, |note| {
                        note.text("Payment terms: net 30 days. Thank you for your business.", small);
                    });
                    row.relative(2.3).background(pale, |background| {
                        background.border(0.8, navy, |border| {
                            border.padding(8.0, |padding| {
                                padding.align(
                                    HorizontalAlign::Right,
                                    VerticalAlign::Top,
                                    |total| {
                                        total.semantic(Semantic::Paragraph, |paragraph| {
                                            paragraph.text(
                                                "Subtotal  19,800.00\nTax  3,960.00\nTOTAL  23,760.00",
                                                body,
                                            );
                                        });
                                    },
                                );
                            });
                        });
                    });
                });
            });
        })
        .build()?;

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output, document.pdf())?;
    println!("{}", output.display());
    Ok(())
}
