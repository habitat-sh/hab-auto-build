# Habitat Auto Build

This is a tool that is designed to assist package managers to speed up the development,
building and testing of large number of inter-related plan files.

## Installation

Install the latest stable version of Rust from https://rustup.rs/.

Check out this repository, then use Cargo to build and install a static `hab-auto-build` binary.

### Linux/macOS

```bash
# Asks the rust compiler to statically link in the C Runtime
export RUSTFLAGS='-C target-feature=+crt-static'
# Explicitly sets the build target, this is required for the C Runtime static linking to work correctly
export CARGO_BUILD_TARGET=$(rustc -vV | grep host | sed 's|host: ||')
cargo install --path .
```

### Windows

```powershell
$env:RUSTFLAGS = '-C target-feature=+crt-static'
$env:CARGO_BUILD_TARGET = (rustc -vV | Select-String 'host:' | ForEach-Object { $_ -replace 'host:\s*', '' }).Trim()
cargo install --path .
```

## Configuration

To configure `hab-auto-build`, you need to create a JSON file with the following structure, which allows the tool to locate and manage plans:

```jsonc
{
    // Multiple repositories can be specified, each containing plans.
    // hab-auto-build will automatically detect cross-repo plan dependencies
    // and build them in the correct order.
    "repos": [
        {
            // A unique ID to identify the set of plans in the 'source' folder
            "id": "core",
            // The path to the folder containing plans (relative or absolute paths allowed).
            // The configuration file does not need to be in the same folder as a plan,
            // enabling you to reference existing source folders containing habitat plans
            // as a repo without any modification.
            "source": "../bootstrap-plans",
            // All plans matching any pattern in the 'native_packages' are considered native packages.
            "native_packages": [
                "packages/bootstrap/build-support/**",
                "packages/bootstrap/build-tools/cross-compiler/**",
                "packages/bootstrap/build-tools/host-tools/**",
                "packages/bootstrap/build-tools/cross-tools/**"
            ],
            // List of patterns for plans that should be ignored by hab-auto-build.
            "ignored_packages": [
                "draft/**"
            ]
        },
        // An additional example repo containing source code and plans for your custom application
        {
            "id": "my-app",
            "source": "/path/to/app/source",
        }
    ]
}
```

This configuration file provides `hab-auto-build` with the necessary information to locate and manage plans across multiple repositories. It ensures that cross-repo dependencies are handled correctly and allows you to include native packages and specify plans to be ignored. The flexibility in the configuration enables seamless integration with existing habitat plans and custom applications.

## Usage

Habitat Auto Build scans all folders and sub-folders within a root repository folder, detecting all plans. By default, it looks for a configuration file named `hab-auto-build.json` in the same folder where you run `hab-auto-build`. To use a different configuration file, you can specify it with the `-c` option.

```bash
# View help and all available options
hab-auto-build --help

# Build all plans
hab-auto-build build
# Perform a dry run of the build to preview the build order
hab-auto-build build -d
# Use a specific configuration file
hab-auto-build build -c /path/to/config

# Build a specific plan and all plans that it depends on
# Example: hab-auto-build build core/build-tools-glibc
hab-auto-build build <plan>

# Check a plan for issues, such as invalid or missing licensing information.
# If an artifact for the plan was built, it will check the artifact for issues.
hab-auto-build check <plan>
```

## Advanced Usage

### Interacting with Git Repositories

Habitat Auto Build, by default, determines if a plan needs a rebuild based on the "last modified" timestamp of any file or directory within the plan context folder. If this timestamp is later than the release timestamp of the last package build, a rebuild is considered necessary. This can sometimes be inconvenient, as file modification times can change due to reasons other than actual content modification. For instance, checking out changes to your plan files from another branch will alter the "last modified" times.

To address this, there are a few options to streamline the process:

```bash
# Build based on the modification times for files using git, not the filesystem
hab-auto-build build -m git
# Synchronize the file "last modified" time with the git commit timestamps
hab-auto-build git-sync
# Dry run of synchronizing the file "last modified" time with the git commit timestamps
hab-auto-build git-sync -d
```

We generally recommend running `hab-auto-build git-sync` after a fresh checkout or when switching branches. This ensures that the need for a rebuild is assessed based on actual changes in the content, not merely due to changes in the file modification time caused by operations like checkout or branch switching.

### Working with multiple plans in commands

Most `hab-auto-build` commands can operate on a list of plans. To specify multiple plans, you can use glob expressions, list each plan name separately, or even combine both methods.

```bash
# Build all the plans starting with core/build-tools
hab-auto-build build core/build-tools-*
# Build multiple plans listed individually
hab-auto-build build core/build-tools-binutils core/build-tools-sed core/build-tools-grep
# Build all the plans starting with core/build-tools, as well as core/gcc and core/binutils
hab-auto-build build core/build-tools-* core/gcc core/binutils
```

These examples demonstrate how to use `hab-auto-build` commands with multiple plans, allowing you to manage and build a variety of plans efficiently.

### Examining Reasons for Plan Rebuilds

Habitat Auto Build keeps track of a list of changes internally, similar to how version control tools like Git manage changes.

To view the list of all plans that have changes and are scheduled for rebuilding, use the following command:

```bash
hab-auto-build changes
```

For a detailed explanation of why each plan needs to be rebuilt, you can use one of the following commands:

```bash
hab-auto-build changes -e
hab-auto-build changes --explain
```

To see the explanation for a specific plan or a group of plans, use the following commands:

```bash
hab-auto-build changes -e <plan>..
# To view changes for core/hab only
hab-auto-build changes -e core/hab
# To view changes for core/hab and all plans for packages starting with core/build-tools
hab-auto-build changes -e core/hab core/build-tools-*
```

These commands allow you to inspect and understand the reasons behind the rebuilding of plans.

### Preventing Rebuilds by Ignoring Plan File Changes

Habitat Auto Build considers a plan for rebuild whenever any source file within the plan context folder changes.

In certain scenarios, you may want to prevent a change from triggering a rebuild, such as formatting changes in a plan. To avoid a rebuild, you can remove the plan from the list of changes using the following command:

```bash
hab-auto-build remove <plan>..
# Removes the core/gcc plan from the change list, preventing a rebuild
hab-auto-build remove core/gcc
```

**Note**
When attempting to remove plans from the change list, you might encounter a situation where the plan depends on other plans that have been rebuilt with a new version. In this case, the plan in question must be rebuilt as well, and cannot be removed with the `hab-auto-build remove` command.

This constraint exists to ensure that rebuilding any plan also rebuilds all its transitive reverse dependencies. In other words, if a plan depends on other plans, which in turn depend on even more plans, all of these plans must be rebuilt whenever a change occurs in any of them.

This process maintains consistency across all reverse dependencies, preventing conflicts and potential issues in the final build that could arise from multiple dependencies using different versions of the same plan.

For example, consider the following plan structure:

```
Plan A
├── Plan B
│   ├── Plan D
│   └── Plan E
└── Plan C
    ├── Plan F
    └── Plan G
```

In this example, Plan A depends on Plan B and Plan C, which in turn depend on other plans (D, E, F, G). If Plan D is rebuilt with a new version, Plan B must be rebuilt as well, since it depends on Plan D. Furthermore, since Plan A depends on Plan B, it would also need to be rebuilt.

This rebuilding process ensures that all reverse dependencies (in this case, Plan B and Plan A) use the same version of Plan D, preventing conflicts or issues that might arise if different versions of Plan D were used.

### Manually Triggering a Plan File Rebuild

There might be cases where you need to force a rebuild of a plan, such as when building native plans where the build outcome depends on the environment. Since Habitat Auto Build cannot automatically detect changes in the environment, you must manually trigger a rebuild by adding the plan to the change list.

```bash
hab-auto-build add <plan>..
# Add the core/native-cross-gcc plan to the change list, forcing a rebuild
hab-auto-build add core/native-cross-gcc
```

By using the `hab-auto-build add` command, you can ensure that the specified plan is rebuilt, accounting for any changes in the environment or other factors that may affect the build outcome. This allows you to maintain consistency and reliability across your habitat environment.

### Configuring Package Violation Checks

Habitat Auto Build performs several checks during the plan building process. One set of checks is carried out on the plan's source files before the build, while another set is performed on the final built artifact. For most packages, these checks help identify any errors that occurred during the build process. However, in some cases, these checks may yield false positives and need to be disabled. You can achieve this by adding a `.hab-plan-config.toml` file alongside your plan file.

Here is a sample `.hab-plan-config.toml` from the binutils package:

```toml
[rules]
# This is used to disable license-based warnings. A specific source SHA is required so that the rule
# exception is disregarded if the sources change. Ideally, a packager should re-evaluate the licensing information
# whenever the sources change.
missing-license = { level = "off", source-shasum = "da24a84fef220102dd24042df06fdea851c2614a5377f86effa28f33b7b16148" }
license-not-found = { level = "off", source-shasum = "da24a84fef220102dd24042df06fdea851c2614a5377f86effa28f33b7b16148" }
# The hab-ld-wrapper binary is used at runtime to process arguments
unused-dependency = { ignored_packages = ["core/hab-ld-wrapper"] }
```

By default, any package check violation will halt the build process. This helps minimize the need for later fixes in the built package, which could trigger a rebuild of all reverse dependencies. However, you can configure this behavior with the `-l`/`--check-level` option:

```bash
# Proceed regardless of package violations
hab-auto-build build -l allow-all
# Proceed if only package warning violations are present
hab-auto-build build -l allow-warnings
# Proceed only if there are no violations
hab-auto-build build -l strict
```
