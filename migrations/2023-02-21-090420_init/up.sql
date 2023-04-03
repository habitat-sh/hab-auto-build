CREATE TABLE file_modifications (
    plan_context_path TEXT NOT NULL,
    file_path TEXT NOT NULL,
    real_modified_at TEXT NOT NULL,
    alternate_modified_at TEXT NOT NULL,
    PRIMARY KEY (plan_context_path, file_path)
);

CREATE TABLE build_times (
    build_ident TEXT NOT NULL,
    duration_in_secs INTEGER NOT NULL,
    PRIMARY KEY (build_ident)
);

CREATE TABLE artifact_contexts (
    hash TEXT NOT NULL,
    context TEXT NOT NULL,
    PRIMARY KEY (hash)
);

CREATE TABLE source_contexts (
    hash TEXT NOT NULL,
    context TEXT NOT NULL,
    PRIMARY KEY (hash)
);