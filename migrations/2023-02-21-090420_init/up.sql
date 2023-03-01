CREATE TABLE file_modifications (
    plan_context_path TEXT NOT NULL,
    file_path TEXT NOT NULL,
    real_modified_at DATETIME NOT NULL,
    alternate_modified_at DATETIME NOT NULL,
    PRIMARY KEY (plan_context_path, file_path)
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