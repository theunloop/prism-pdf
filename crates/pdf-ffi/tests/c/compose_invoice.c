/* A real C consumer for the M25 acceptance invoice. No Rust callbacks are used. */
#include "prismpdf.h"

#include <stdio.h>

#define CHECK(call) do { status = (call); if (status != PrismPdfStatus_Ok) goto cleanup; } while (0)
#define TRACK(handle) do { handles[handle_count++] = (handle); } while (0)

static PrismPdfStatus set_text(PrismPdfCompositionContainer *slot,
                              const char *text,
                              PrismPdfCompositionTextStyle style) {
    return prismpdf_composition_container_set_text(slot, text, &style);
}

static PrismPdfStatus add_text_cell(PrismPdfCompositionContainer *row,
                                   const char *text,
                                   PrismPdfCompositionTextStyle style,
                                   PrismPdfCompositionContainer **handles,
                                   size_t *handle_count) {
    PrismPdfCompositionContainer *cell = NULL;
    PrismPdfStatus status = prismpdf_composition_table_row_add_cell(row, &cell);
    if (status != PrismPdfStatus_Ok) return status;
    handles[(*handle_count)++] = cell;
    return set_text(cell, text, style);
}

/* Returns library-owned bytes; the caller releases them with prismpdf_bytes_free. */
int prismpdf_c_invoice_acceptance(uint8_t **out_data, uintptr_t *out_len) {
    PrismPdfStatus status = PrismPdfStatus_Internal;
    PrismPdfComposition *composition = prismpdf_composition_new();
    PrismPdfCompositionContainer *handles[512];
    size_t handle_count = 0;
    PrismPdfCompositionContainer *content = NULL, *column = NULL, *slot = NULL;
    PrismPdfCompositionContainer *row = NULL, *child = NULL, *table = NULL;
    PrismPdfCompositionContainer *header = NULL, *footer = NULL;
    PrismPdfCompositionPageStyle page = {595.0, 842.0, 42.0, 42.0, 42.0, 42.0};
    PrismPdfCompositionTextStyle small = {9.0, 12.0};
    PrismPdfCompositionTextStyle body = {10.0, 14.0};
    PrismPdfCompositionTextStyle heading = {20.0, 24.0};
    PrismPdfCompositionColor pale = {0.93, 0.96, 0.99};
    char item[24], description[64], quantity[8], amount[24];
    int index;

    if (composition == NULL || out_data == NULL || out_len == NULL) goto cleanup;
    *out_data = NULL;
    *out_len = 0;
    CHECK(prismpdf_composition_set_tagged_language(composition, "en-US"));
    CHECK(prismpdf_composition_add_page(composition, &page, &content)); TRACK(content);

    CHECK(prismpdf_composition_page_set_header(composition, 0, &header)); TRACK(header);
    CHECK(prismpdf_composition_container_set_background(header, pale, &child)); TRACK(child);
    CHECK(prismpdf_composition_container_set_padding(child, 8.0, &slot)); TRACK(slot);
    CHECK(set_text(slot, "Prism PDF Studio                         Invoice 2026-0042", small));

    CHECK(prismpdf_composition_page_set_footer(composition, 0, &footer)); TRACK(footer);
    CHECK(prismpdf_composition_container_set_alignment(
        footer, PrismPdfCompositionHorizontalAlign_Center,
        PrismPdfCompositionVerticalAlign_Top, &child)); TRACK(child);
    CHECK(set_text(child, "Page {page} of {pages}", small));

    CHECK(prismpdf_composition_container_set_column(content, 12.0, &column)); TRACK(column);
    CHECK(prismpdf_composition_column_add_item(column, &slot)); TRACK(slot);
    CHECK(prismpdf_composition_container_set_row(slot, &row)); TRACK(row);
    CHECK(prismpdf_composition_row_add_relative(row, 1.0, &slot)); TRACK(slot);
    CHECK(prismpdf_composition_container_set_heading(slot, 1, &child)); TRACK(child);
    CHECK(set_text(child, "INVOICE", heading));
    CHECK(prismpdf_composition_row_add_relative(row, 1.0, &slot)); TRACK(slot);
    CHECK(prismpdf_composition_container_set_alignment(
        slot, PrismPdfCompositionHorizontalAlign_Right,
        PrismPdfCompositionVerticalAlign_Top, &child)); TRACK(child);
    CHECK(set_text(child, "No. 2026-0042\nIssued: 24 Aug 2026\nDue: 23 Sep 2026", small));

    CHECK(prismpdf_composition_column_add_item(column, &slot)); TRACK(slot);
    CHECK(prismpdf_composition_container_set_table(slot, &table)); TRACK(table);
    CHECK(prismpdf_composition_table_add_relative_column(table, 0.8));
    CHECK(prismpdf_composition_table_add_relative_column(table, 4.2));
    CHECK(prismpdf_composition_table_add_relative_column(table, 1.0));
    CHECK(prismpdf_composition_table_add_relative_column(table, 1.3));
    CHECK(prismpdf_composition_table_set_header(table, &row)); TRACK(row);
    CHECK(add_text_cell(row, "ITEM", small, handles, &handle_count));
    CHECK(add_text_cell(row, "DESCRIPTION", small, handles, &handle_count));
    CHECK(add_text_cell(row, "QTY", small, handles, &handle_count));
    CHECK(add_text_cell(row, "AMOUNT", small, handles, &handle_count));

    for (index = 1; index <= 64; ++index) {
        CHECK(prismpdf_composition_table_add_row(table, &row)); TRACK(row);
        (void)snprintf(item, sizeof item, "%02d", index);
        (void)snprintf(description, sizeof description, "Document processing service %02d", index);
        (void)snprintf(quantity, sizeof quantity, "%d", index % 4 + 1);
        (void)snprintf(amount, sizeof amount, "%.2f", 37.5 * index);
        CHECK(add_text_cell(row, item, small, handles, &handle_count));
        CHECK(add_text_cell(row, description, small, handles, &handle_count));
        CHECK(add_text_cell(row, quantity, small, handles, &handle_count));
        CHECK(add_text_cell(row, amount, small, handles, &handle_count));
    }

    CHECK(prismpdf_composition_column_add_item(column, &slot)); TRACK(slot);
    CHECK(prismpdf_composition_container_set_background(slot, pale, &child)); TRACK(child);
    CHECK(prismpdf_composition_container_set_padding(child, 8.0, &slot)); TRACK(slot);
    CHECK(set_text(slot,
        "Payment terms: net 30 days.  Subtotal 19,800.00  Tax 3,960.00  TOTAL 23,760.00",
        body));
    status = prismpdf_composition_build(composition, out_data, out_len);

cleanup:
    while (handle_count > 0) {
        prismpdf_composition_container_free(handles[--handle_count]);
    }
    prismpdf_composition_free(composition);
    return (int)status;
}
