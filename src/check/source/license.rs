use std::{
    collections::{BTreeSet, HashMap},
    fmt::Display,
    path::PathBuf,
};

use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};

use crate::{
    check::{
        ContextRules, LeveledSourceCheckViolation, SourceCheck, SourceCheckViolation,
        SourceRuleOptions, ViolationLevel,
    },
    core::{ArtifactContext, Blake3, PackageSha256Sum, PlanContext, SourceContext},
};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "rule", content = "metadata")]
pub(crate) enum LicenseRule {
    #[serde(rename = "missing-license")]
    MissingLicense(MissingLicense),
    #[serde(rename = "license-not-found")]
    LicenseNotFound(LicenseNotFound),
    #[serde(rename = "invalid-license-expression")]
    InvalidLicenseExpression(InvalidLicenseExpression),
}

impl Display for LicenseRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LicenseRule::MissingLicense(rule) => write!(f, "{}", rule),
            LicenseRule::LicenseNotFound(rule) => write!(f, "{}", rule),
            LicenseRule::InvalidLicenseExpression(rule) => write!(f, "{}", rule),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "id", content = "options")]
pub(crate) enum LicenseRuleOptions {
    #[serde(rename = "missing-license")]
    MissingLicense(MissingLicenseOptions),
    #[serde(rename = "license-not-found")]
    LicenseNotFound(LicenseNotFoundOptions),
    #[serde(rename = "invalid-license-expression")]
    InvalidLicenseExpression(InvalidLicenseExpressionOptions),
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingLicense {
    pub license: String,
    pub sources: BTreeSet<PathBuf>,
    pub source_shasum: Option<PackageSha256Sum>,
}

impl Display for MissingLicense {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(source_shasum) = &self.source_shasum {
            write!(
                f,
                "Found license '{}' in files with source-shasum='{}':\n{}",
                self.license.yellow(),
                source_shasum.blue(),
                self.sources
                    .iter()
                    .map(|p| format!("                  - {}", p.display().blue()))
                    .collect::<Vec<String>>()
                    .join("\n"),
            )
        } else {
            write!(
                f,
                "Found license '{}' in files:\n{}",
                self.license.yellow(),
                self.sources
                    .iter()
                    .map(|p| format!("                  - {}", p.display().blue()))
                    .collect::<Vec<String>>()
                    .join("\n"),
            )
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingLicenseOptions {
    pub level: ViolationLevel,
    #[serde(default, rename = "source-shasum")]
    pub source_shasum: Option<PackageSha256Sum>,
}

impl Default for MissingLicenseOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
            source_shasum: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LicenseNotFound {
    pub license: String,
    pub source_shasum: Option<PackageSha256Sum>,
}

impl Display for LicenseNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(source_shasum) = &self.source_shasum {
            write!(
                f,
                "License '{}' specified in the 'pkg_licenses' not found in source with source-shasum='{}'",
                self.license.yellow(),
                source_shasum.blue()
            )
        } else {
            write!(
                f,
                "License '{}' specified in the 'pkg_licenses' not found in source",
                self.license.yellow(),
            )
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct LicenseNotFoundOptions {
    pub level: ViolationLevel,
    #[serde(default, rename = "source-shasum")]
    pub source_shasum: Option<PackageSha256Sum>,
}

impl Default for LicenseNotFoundOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
            source_shasum: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct InvalidLicenseExpression {
    pub expression: String,
    pub err: String,
}

impl Display for InvalidLicenseExpression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "License expression '{}' is not valid: {}",
            self.expression.yellow(),
            self.err
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct InvalidLicenseExpressionOptions {
    pub level: ViolationLevel,
}

impl Default for InvalidLicenseExpressionOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Error,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct LicenseCheck {}

impl LicenseCheck {
    fn source_context_check(
        &self,
        rules: &ContextRules,
        license_expressions: &[String],
        source_context: &SourceContext,
    ) -> Vec<LeveledSourceCheckViolation> {
        let mut violations = Vec::new();
        let mut specified_licenses = BTreeSet::new();
        let mut detected_licenses = BTreeSet::default();
        let mut license_sources: HashMap<String, BTreeSet<PathBuf>> = HashMap::new();

        let missing_license_options = rules
            .source_rules
            .iter()
            .filter_map(|rule| {
                if let SourceRuleOptions::License(LicenseRuleOptions::MissingLicense(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");
        let license_not_found_options = rules
            .source_rules
            .iter()
            .filter_map(|rule| {
                if let SourceRuleOptions::License(LicenseRuleOptions::LicenseNotFound(options)) =
                    &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");
        let invalid_license_expression_options = rules
            .source_rules
            .iter()
            .filter_map(|rule| {
                if let SourceRuleOptions::License(LicenseRuleOptions::InvalidLicenseExpression(
                    options,
                )) = &rule.options
                {
                    Some(options)
                } else {
                    None
                }
            })
            .last()
            .expect("Default rule missing");

        for license_expression in license_expressions {
            match spdx::Expression::parse(&license_expression) {
                Ok(expression) => {
                    for req_expression in expression.requirements() {
                        // Transform license id into correct string form
                        let license_id = match &req_expression.req.license {
                            spdx::LicenseItem::Spdx {
                                id,
                                or_later: false,
                            } => {
                                if id.is_gnu() {
                                    format!("{}-only", id.name)
                                } else {
                                    id.name.to_string()
                                }
                            }
                            spdx::LicenseItem::Spdx { id, or_later: true } => {
                                if id.is_gnu() {
                                    format!("{}-or-later", id.name)
                                } else {
                                    format!("{}+", id.name)
                                }
                            }
                            spdx::LicenseItem::Other {
                                doc_ref: _,
                                lic_ref,
                            } => {
                                format!("{}", lic_ref)
                            }
                        };
                        if !specified_licenses.contains(&license_id) {
                            specified_licenses.insert(license_id);
                        }
                        if let Some(ref exception) = req_expression.req.exception {
                            if !specified_licenses.contains(exception.name) {
                                specified_licenses.insert(exception.name.to_string());
                            }
                        }
                    }
                }
                Err(err) => {
                    violations.push(LeveledSourceCheckViolation {
                        level: invalid_license_expression_options.level,
                        violation: SourceCheckViolation::License(
                            LicenseRule::InvalidLicenseExpression(InvalidLicenseExpression {
                                expression: license_expression.clone(),
                                err: err.reason.to_string(),
                            }),
                        ),
                    });
                }
            }
        }

        for license_ctx in source_context.licenses.iter() {
            for detected_license in license_ctx.detected_licenses.iter() {
                license_sources
                    .entry(detected_license.clone())
                    .or_default()
                    .insert(license_ctx.path.clone());
            }
            detected_licenses.extend(license_ctx.detected_licenses.clone().into_iter());
        }

        let missing_licenses = detected_licenses.difference(&specified_licenses);
        for missing_license in missing_licenses {
            violations.push(LeveledSourceCheckViolation {
                level: match (
                    &source_context.source_shasum,
                    &missing_license_options.source_shasum,
                ) {
                    (Some(source_shasum), Some(options_source_shasum))
                        if source_shasum == options_source_shasum =>
                    {
                        missing_license_options.level
                    }
                    (None, _) => missing_license_options.level,
                    _ => ViolationLevel::Error,
                },
                violation: SourceCheckViolation::License(LicenseRule::MissingLicense(
                    MissingLicense {
                        sources: license_sources.remove(missing_license).unwrap(),
                        license: missing_license.clone(),
                        source_shasum: source_context.source_shasum.clone(),
                    },
                )),
            });
        }

        let licenses_not_found = specified_licenses.difference(&detected_licenses);
        for license_not_found in licenses_not_found {
            violations.push(LeveledSourceCheckViolation {
                level: match (
                    &source_context.source_shasum,
                    &license_not_found_options.source_shasum,
                ) {
                    (Some(source_shasum), Some(options_source_shasum))
                        if source_shasum == options_source_shasum =>
                    {
                        license_not_found_options.level
                    }
                    (None, _) => license_not_found_options.level,
                    _ => ViolationLevel::Error,
                },
                violation: SourceCheckViolation::License(LicenseRule::LicenseNotFound(
                    LicenseNotFound {
                        license: license_not_found.clone(),
                        source_shasum: source_context.source_shasum.clone(),
                    },
                )),
            });
        }

        violations.into_iter().collect()
    }
}

impl SourceCheck for LicenseCheck {
    fn source_context_check_with_plan(
        &self,
        rules: &ContextRules,
        plan_context: &PlanContext,
        source_context: &SourceContext,
    ) -> Vec<LeveledSourceCheckViolation> {
        LicenseCheck::source_context_check(&self, rules, &plan_context.licenses, source_context)
    }

    fn source_context_check_with_artifact(
        &self,
        rules: &ContextRules,
        artifact_context: &ArtifactContext,
        source_context: &SourceContext,
    ) -> Vec<LeveledSourceCheckViolation> {
        LicenseCheck::source_context_check(&self, rules, &artifact_context.licenses, source_context)
    }
}
