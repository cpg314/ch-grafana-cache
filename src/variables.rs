use std::collections::HashMap;

use itertools::Itertools;

lazy_static::lazy_static! {
    pub static ref VARIABLE_RE: regex::Regex = regex::Regex::new(r#"\$\{(.*?)\}"#).unwrap();
}

pub type VariablesAssignment<'a> = HashMap<&'a str, String>;

#[derive(thiserror::Error, Debug)]
enum SubsError {
    #[error("Variable {0} not found")]
    NotFound(String),
}

fn substitute_variable(
    cap: &regex::Captures,
    variables: &VariablesAssignment<'_>,
) -> Result<String, SubsError> {
    let name = cap.get(1).unwrap().as_str();
    variables
        .get(name)
        .ok_or_else(|| SubsError::NotFound(name.into()))
        .cloned()
        .map(|s| s.to_string())
}
pub fn substitute_variables(
    sql: &str,
    variables: &VariablesAssignment<'_>,
) -> anyhow::Result<String> {
    let mut errors = Vec::<SubsError>::default();

    let sql = VARIABLE_RE
        .replace_all(sql, |cap: &regex::Captures| {
            substitute_variable(cap, variables).unwrap_or_else(|e| {
                errors.push(e);
                "ERROR".into()
            })
        })
        .into_owned();
    if !errors.is_empty() {
        anyhow::bail!(
            "Encountered substitution errors:\n  {}",
            errors.into_iter().map(|e| e.to_string()).join("\n  ")
        );
    }
    Ok(sql)
}
