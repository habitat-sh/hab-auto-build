# Habitat Auto Build

This is a tool that is designed to assist package managers to speed up the development,
building and testing of large number of inter-related plan files.

## Installation

Check out this repository. Then use cargo to build and install the `hab-auto-build` binary.

```bash
# Inside the hab-auto-build codebase
cargo install --path .
```

## Usage

Habitat Auto Build scans all folders and sub-folders within a root repository folder and detects all plans.

```bash
# Build all plans
hab-auto-build build

# Build a specific plan and all plans that depend on it
hab-auto-build build <plan>
# Eg: hab-auto-build build core/build-tools-glibc
```