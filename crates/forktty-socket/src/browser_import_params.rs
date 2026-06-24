use forktty_core::{ProfileId, ProfileStore};
use serde_json::Value;

use crate::DispatchError;

#[derive(Debug, Clone, Copy)]
pub(crate) struct BrowserImportSelection {
    pub(crate) history: bool,
    pub(crate) bookmarks: bool,
    pub(crate) cookies: bool,
}

impl BrowserImportSelection {
    fn any(self) -> bool {
        self.history || self.bookmarks || self.cookies
    }

    pub(crate) fn read_selection(self) -> forktty_import::ImportReadSelection {
        forktty_import::ImportReadSelection {
            cookies: self.cookies,
            history: self.history,
            bookmarks: self.bookmarks,
        }
    }
}

pub(crate) struct BrowserImportPreviewRequest {
    pub(crate) include: BrowserImportSelection,
    pub(crate) sources: Vec<forktty_import::SourceProfile>,
}

impl BrowserImportPreviewRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        let include = browser_import_selection(params)?;
        let sources = browser_import_selected_sources(params)?;
        Ok(Self { include, sources })
    }
}

pub(crate) struct BrowserImportRunRequest {
    pub(crate) include: BrowserImportSelection,
    sources: Vec<forktty_import::SourceProfile>,
}

impl BrowserImportRunRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        let include = browser_import_selection(params)?;
        let sources = browser_import_selected_sources(params)?;
        Ok(Self { include, sources })
    }

    pub(crate) fn plan(
        &self,
        params: &Value,
        store: &ProfileStore,
    ) -> Result<forktty_import::ImportPlan, DispatchError> {
        browser_import_plan_from_params(params, &self.sources, store)
    }
}

fn browser_family_key(family: forktty_import::BrowserFamily) -> &'static str {
    match family {
        forktty_import::BrowserFamily::Firefox => "firefox",
        forktty_import::BrowserFamily::Chrome => "chrome",
        forktty_import::BrowserFamily::Chromium => "chromium",
        forktty_import::BrowserFamily::Brave => "brave",
        forktty_import::BrowserFamily::Edge => "edge",
        forktty_import::BrowserFamily::Vivaldi => "vivaldi",
    }
}

pub(crate) fn browser_import_source_id(profile: &forktty_import::SourceProfile) -> String {
    format!("{}:{}", browser_family_key(profile.family), profile.path)
}

fn browser_import_bool_param(
    value: Option<&Value>,
    key: &'static str,
    default: bool,
) -> Result<bool, DispatchError> {
    match value {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(DispatchError::InvalidParam(format!(
            "Invalid parameter {key}: expected boolean"
        ))),
    }
}

fn browser_import_selection(params: &Value) -> Result<BrowserImportSelection, DispatchError> {
    let Some(include) = params.get("include") else {
        return Ok(BrowserImportSelection {
            history: true,
            bookmarks: true,
            cookies: true,
        });
    };
    let object = include.as_object().ok_or_else(|| {
        DispatchError::InvalidParam("Invalid parameter include: expected object".to_string())
    })?;
    let selection = BrowserImportSelection {
        history: browser_import_bool_param(object.get("history"), "include.history", true)?,
        bookmarks: browser_import_bool_param(object.get("bookmarks"), "include.bookmarks", true)?,
        cookies: browser_import_bool_param(object.get("cookies"), "include.cookies", true)?,
    };
    if !selection.any() {
        return Err(DispatchError::InvalidParam(
            "select at least one browser data type to import".to_string(),
        ));
    }
    Ok(selection)
}

fn browser_import_all_sources_param(params: &Value) -> Result<bool, DispatchError> {
    browser_import_bool_param(params.get("all"), "all", false)
}

fn browser_import_selected_sources(
    params: &Value,
) -> Result<Vec<forktty_import::SourceProfile>, DispatchError> {
    let all = browser_import_all_sources_param(params)?;
    let discovered: Vec<forktty_import::SourceProfile> = forktty_import::discover()
        .into_iter()
        .flat_map(|browser| browser.profiles.into_iter())
        .collect();
    if all {
        if params
            .get("sources")
            .is_some_and(|sources| !sources.is_null())
        {
            return Err(DispatchError::InvalidParam(
                "cannot combine all and sources".to_string(),
            ));
        }
        return Ok(discovered);
    }

    let Some(sources) = params.get("sources") else {
        return Err(DispatchError::MissingParam("sources"));
    };
    let ids = sources.as_array().ok_or_else(|| {
        DispatchError::InvalidParam("Invalid parameter sources: expected array".to_string())
    })?;
    if ids.is_empty() {
        return Err(DispatchError::InvalidParam(
            "sources must not be empty".to_string(),
        ));
    }

    let mut selected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for value in ids {
        let id = value.as_str().ok_or_else(|| {
            DispatchError::InvalidParam("Invalid parameter sources: expected strings".to_string())
        })?;
        if id.trim().is_empty() {
            return Err(DispatchError::InvalidParam(
                "sources must not include empty source ids".to_string(),
            ));
        }
        if !seen.insert(id.to_string()) {
            continue;
        }
        let Some(profile) = discovered
            .iter()
            .find(|profile| browser_import_source_id(profile) == id)
        else {
            return Err(DispatchError::NotFound("browser import source".to_string()));
        };
        selected.push(profile.clone());
    }
    Ok(selected)
}

fn resolve_profile_value_in_store(
    store: &ProfileStore,
    value: &Value,
) -> Result<ProfileId, DispatchError> {
    let profile = value.as_str().ok_or_else(|| {
        DispatchError::InvalidParam("Invalid parameter profile: expected string".to_string())
    })?;
    store
        .resolve(profile)
        .ok_or(DispatchError::NotFound("profile".to_string()))
}

fn optional_profile_param_in_store(
    store: &ProfileStore,
    params: &Value,
) -> Result<ProfileId, DispatchError> {
    match params.get("profile") {
        None | Some(Value::Null) => Ok(ProfileId::default()),
        Some(value) => resolve_profile_value_in_store(store, value),
    }
}

fn browser_import_destination_from_params(
    params: &Value,
    store: &ProfileStore,
) -> Result<Option<forktty_import::ImportDestination>, DispatchError> {
    let Some(destination) = params.get("destination") else {
        return Ok(None);
    };
    let object = destination.as_object().ok_or_else(|| {
        DispatchError::InvalidParam("Invalid parameter destination: expected object".to_string())
    })?;
    let kind = match object.get("kind") {
        None | Some(Value::Null) => return Err(DispatchError::MissingParam("destination.kind")),
        Some(Value::String(kind)) => kind.as_str(),
        Some(_) => {
            return Err(DispatchError::InvalidParam(
                "Invalid parameter destination.kind: expected string".to_string(),
            ));
        }
    };
    match kind {
        "existing" => {
            let value = object
                .get("profile")
                .or_else(|| object.get("id"))
                .ok_or(DispatchError::MissingParam("destination.profile"))?;
            Ok(Some(forktty_import::ImportDestination::Existing(
                resolve_profile_value_in_store(store, value)?,
            )))
        }
        "create" => {
            let name = match object.get("display_name").or_else(|| object.get("name")) {
                None | Some(Value::Null) => {
                    return Err(DispatchError::MissingParam("destination.display_name"));
                }
                Some(Value::String(name)) => name.trim().to_string(),
                Some(_) => {
                    return Err(DispatchError::InvalidParam(
                        "Invalid parameter destination.display_name: expected string".to_string(),
                    ));
                }
            };
            if name.is_empty() {
                return Err(DispatchError::InvalidParam(
                    "destination.display_name must not be empty".to_string(),
                ));
            }
            Ok(Some(forktty_import::ImportDestination::Create(name)))
        }
        other => Err(DispatchError::InvalidParam(format!(
            "Invalid parameter destination.kind: expected existing or create, got {other}"
        ))),
    }
}

fn browser_import_plan_from_params(
    params: &Value,
    selected: &[forktty_import::SourceProfile],
    store: &ProfileStore,
) -> Result<forktty_import::ImportPlan, DispatchError> {
    if let Some(destination) = browser_import_destination_from_params(params, store)? {
        return Ok(forktty_import::ImportPlan {
            mode: forktty_import::ImportMode::SingleDestination,
            entries: vec![forktty_import::ImportEntry {
                sources: selected.to_vec(),
                destination,
            }],
        });
    }

    let mode = match params.get("mode") {
        None | Some(Value::Null) => "default",
        Some(Value::String(mode)) => mode.as_str(),
        Some(_) => {
            return Err(DispatchError::InvalidParam(
                "Invalid parameter mode: expected string".to_string(),
            ));
        }
    };
    match mode {
        "default" => {
            let preferred = optional_profile_param_in_store(store, params)?;
            Ok(forktty_import::resolve_default_plan(
                selected,
                store.list(),
                preferred,
            ))
        }
        "single_destination" => {
            let profile = optional_profile_param_in_store(store, params)?;
            Ok(forktty_import::ImportPlan {
                mode: forktty_import::ImportMode::SingleDestination,
                entries: vec![forktty_import::ImportEntry {
                    sources: selected.to_vec(),
                    destination: forktty_import::ImportDestination::Existing(profile),
                }],
            })
        }
        "separate_profiles" => Ok(forktty_import::resolve_separate_profiles_plan(
            selected,
            store.list(),
        )),
        other => Err(DispatchError::InvalidParam(format!(
            "Invalid parameter mode: expected default, single_destination, or separate_profiles, got {other}"
        ))),
    }
}
