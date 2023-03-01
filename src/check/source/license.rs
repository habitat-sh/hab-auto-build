use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
};

use askalono::{ScanMode, ScanStrategy, Store, TextData};
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};

use crate::{
    check::{
        ContextRules, LeveledSourceCheckViolation, SourceCheck, SourceCheckViolation, SourceRule,
        SourceRuleOptions, ViolationLevel,
    },
    core::{ArtifactContext, PlanContext, SourceContext},
};

const LICENSE_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/license-cache.bin.gz"));
const DEPRECATED_LICENSE_DATA: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/deprecated-license-cache.bin.gz"));

lazy_static! {
    static ref LICENSE_STORE: Store = Store::from_cache(LICENSE_DATA).unwrap();
    static ref DEPRECATED_LICENSE_STORE: Store =
        Store::from_cache(DEPRECATED_LICENSE_DATA).unwrap();
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "rule", content = "metadata")]
pub(crate) enum LicenseRule {
    #[serde(rename = "missing-license")]
    MissingLicense(MissingLicense),
    #[serde(rename = "license-not-found")]
    LicenseNotFound(LicenseNotFound),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "id", content = "options")]
pub(crate) enum LicenseRuleOptions {
    #[serde(rename = "missing-license")]
    MissingLicense(MissingLicenseOptions),
    #[serde(rename = "license-not-found")]
    LicenseNotFound(LicenseNotFoundOptions),
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct MissingLicense {
    pub license: String,
    pub sources: Vec<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct MissingLicenseOptions {
    pub level: ViolationLevel,
    #[serde(default)]
    pub ignore_licenses: HashSet<String>,
}

impl Default for MissingLicenseOptions {
    fn default() -> Self {
        Self {
            level: ViolationLevel::Warn,
            ignore_licenses: HashSet::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LicenseNotFound {
    pub license: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct LicenseNotFoundOptions {
    pub level: ViolationLevel,
}

impl Default for LicenseNotFoundOptions {
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
        specified_licenses: &[String],
        source_context: &SourceContext,
    ) -> Vec<LeveledSourceCheckViolation> {
        let mut violations = Vec::new();
        let specified_licenses = specified_licenses.iter().cloned().collect::<BTreeSet<_>>();
        let mut detected_licenses = BTreeSet::default();
        let mut license_sources: HashMap<String, Vec<PathBuf>> = HashMap::new();

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

        let deep_scan_strategies = vec![
            ScanStrategy::new(&*LICENSE_STORE)
                .confidence_threshold(0.8)
                .mode(ScanMode::TopDown)
                .max_passes(50)
                .optimize(true),
            ScanStrategy::new(&*DEPRECATED_LICENSE_STORE)
                .confidence_threshold(0.8)
                .mode(ScanMode::TopDown)
                .max_passes(50)
                .optimize(true),
        ];
        for license_ctx in source_context.licenses.iter() {
            let data = TextData::from(license_ctx.text.clone());
            let mut file_licenses = BTreeSet::new();
            // Do a more costly scan for licenses if we find a lot of them
            for strategy in deep_scan_strategies.iter() {
                if let Ok(results) = strategy.scan(&data) {
                    for item in results.containing {
                        file_licenses.insert(item.license.name.to_string());
                        license_sources
                            .entry(item.license.name.to_string())
                            .or_default()
                            .push(license_ctx.path.clone());
                    }
                }
            }
            detected_licenses.append(&mut file_licenses);
        }

        let missing_licenses = detected_licenses.difference(&specified_licenses);

        for missing_license in missing_licenses {
            if missing_license_options
                .ignore_licenses
                .contains(missing_license)
            {
                continue;
            }
            violations.push(LeveledSourceCheckViolation {
                level: missing_license_options.level,
                violation: SourceCheckViolation::License(LicenseRule::MissingLicense(
                    MissingLicense {
                        sources: license_sources.remove(missing_license).unwrap(),
                        license: missing_license.clone(),
                    },
                )),
            });
        }

        let licenses_not_found = specified_licenses.difference(&detected_licenses);
        for license_not_found in licenses_not_found {
            violations.push(LeveledSourceCheckViolation {
                level: license_not_found_options.level,
                violation: SourceCheckViolation::License(LicenseRule::LicenseNotFound(
                    LicenseNotFound {
                        license: license_not_found.clone(),
                    },
                )),
            });
        }

        violations
            .into_iter()
            .filter(|v| v.level != ViolationLevel::Off)
            .collect()
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
