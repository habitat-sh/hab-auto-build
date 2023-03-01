// @generated automatically by Diesel CLI.

diesel::table! {
    file_modifications (plan_context_path, file_path) {
        plan_context_path -> Text,
        file_path -> Text,
        real_modified_at -> Timestamp,
        alternate_modified_at -> Timestamp,
    }
}

diesel::table! {
    artifact_contexts (hash) {
        hash -> Text,
        context -> Text,
    }
}

diesel::table! {
    source_contexts (hash) {
        hash -> Text,
        context -> Text,
    }
}