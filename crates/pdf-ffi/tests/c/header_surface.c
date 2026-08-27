#include "prismpdf.h"

/* Compile-only coverage for API families whose implementation tests call Rust exports directly. */
void prismpdf_header_surface(void) {
    (void)&prismpdf_last_error;
    (void)&prismpdf_error_info_status;
    (void)&prismpdf_error_info_message;
    (void)&prismpdf_error_info_free;

    (void)&prismpdf_object_new_null;
    (void)&prismpdf_object_new_boolean;
    (void)&prismpdf_object_new_integer;
    (void)&prismpdf_object_new_real;
    (void)&prismpdf_object_new_string;
    (void)&prismpdf_object_new_name;
    (void)&prismpdf_object_new_array;
    (void)&prismpdf_object_new_dictionary;
    (void)&prismpdf_object_new_reference;
    (void)&prismpdf_object_new_stream;
    (void)&prismpdf_object_boolean;
    (void)&prismpdf_object_integer;
    (void)&prismpdf_object_real;
    (void)&prismpdf_edit_new;
    (void)&prismpdf_edit_set_object;
    (void)&prismpdf_edit_commit;

    (void)&prismpdf_struct_node_new;
    (void)&prismpdf_struct_node_set_alt;
    (void)&prismpdf_struct_node_set_actual_text;
    (void)&prismpdf_struct_node_set_lang;
    (void)&prismpdf_struct_node_set_namespace;
    (void)&prismpdf_struct_node_set_id;
    (void)&prismpdf_struct_node_add_child;
    (void)&prismpdf_builder_add_structure_node;
}
