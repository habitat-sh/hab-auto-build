// @generated automatically by Diesel CLI.

diesel::table! {
    artifact_contexts (hash) {
        hash -> Text,
        context -> Text,
    }
}

diesel::table! {
    file_modifications (plan_context_path, file_path) {
        plan_context_path -> Text,
        file_path -> Text,
        real_modified_at -> Text,
        alternate_modified_at -> Text,
    }
}

diesel::table! {
    build_times (build_ident) {
        build_ident -> Text,
        duration_in_secs -> Integer,
    }
}

diesel::table! {
    source_contexts (hash) {
        hash -> Text,
        context -> Text,
    }
}

diesel::allow_tables_to_appear_in_same_query!(
    artifact_contexts,
    file_modifications,
    source_contexts,
);
