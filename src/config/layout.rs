//! `layout` path templates (design D2): a hand-rolled two-token
//! mini-language (`Literal` / `Field`) parsed once at config load, so a
//! typo surfaces at startup instead of mid-import (spec: "Layout
//! templates validated at load").

use std::collections::HashMap;

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::error::{Error, Result};

/// The one field name core understands: it formats from the group's
/// timestamp via jiff's strftime. Any other field name is looked up in
/// the group's `context` map — the vocabulary there is deliberately
/// left to device modules to define (design Open Questions).
const DATE_FIELD: &str = "date";
const DEFAULT_DATE_FORMAT: &str = "%Y-%m-%d";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Literal(String),
    Field {
        name: String,
        strftime: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutTemplate {
    tokens: Vec<Token>,
}

impl LayoutTemplate {
    /// Parses `input` into a token list, validating brace matching,
    /// field names, and (for `{date...}`) the strftime spec eagerly so
    /// invalid templates fail here rather than at plan-building time.
    pub fn parse(input: &str) -> Result<Self> {
        let chars: Vec<char> = input.chars().collect();
        let mut tokens = Vec::new();
        let mut literal = String::new();
        let mut i = 0;

        while i < chars.len() {
            match chars[i] {
                '{' => {
                    if !literal.is_empty() {
                        tokens.push(Token::Literal(std::mem::take(&mut literal)));
                    }
                    let start = i;
                    i += 1;
                    let mut field = String::new();
                    let mut closed = false;
                    while i < chars.len() {
                        if chars[i] == '}' {
                            closed = true;
                            break;
                        }
                        field.push(chars[i]);
                        i += 1;
                    }
                    if !closed {
                        return Err(Error::Template(format!(
                            "unclosed '{{' at position {start}"
                        )));
                    }
                    i += 1; // consume '}'

                    let (name, strftime) = match field.split_once(':') {
                        Some((n, f)) => (n.to_string(), Some(f.to_string())),
                        None => (field.clone(), None),
                    };
                    if name.is_empty() {
                        return Err(Error::Template(format!(
                            "empty field name at position {start}"
                        )));
                    }
                    if name == DATE_FIELD {
                        let fmt = strftime.as_deref().unwrap_or(DEFAULT_DATE_FORMAT);
                        jiff::fmt::strtime::format(fmt, Timestamp::UNIX_EPOCH).map_err(|e| {
                            Error::Template(format!(
                                "invalid strftime spec at position {start}: {e}"
                            ))
                        })?;
                    }
                    tokens.push(Token::Field { name, strftime });
                }
                '}' => {
                    return Err(Error::Template(format!("unmatched '}}' at position {i}")));
                }
                c => {
                    literal.push(c);
                    i += 1;
                }
            }
        }
        if !literal.is_empty() {
            tokens.push(Token::Literal(literal));
        }
        Ok(LayoutTemplate { tokens })
    }

    /// Resolves the template against a group's context map and
    /// timestamp. `{date...}` formats `timestamp` in `tz` via jiff's
    /// strftime — the zone is applied here so late-evening instants
    /// land on the local calendar day (design D4). Every other field
    /// is looked up by name in `context`.
    pub fn resolve(
        &self,
        context: &HashMap<String, String>,
        timestamp: Timestamp,
        tz: &TimeZone,
    ) -> Result<String> {
        let mut out = String::new();
        for token in &self.tokens {
            match token {
                Token::Literal(s) => out.push_str(s),
                Token::Field { name, strftime } if name == DATE_FIELD => {
                    let fmt = strftime.as_deref().unwrap_or(DEFAULT_DATE_FORMAT);
                    let zoned = timestamp.to_zoned(tz.clone());
                    let formatted = jiff::fmt::strtime::format(fmt, &zoned)
                        .expect("strftime spec validated at parse time");
                    out.push_str(&formatted);
                }
                Token::Field { name, .. } => {
                    let value = context.get(name).ok_or_else(|| {
                        Error::Template(format!("no value for field '{name}' in this group"))
                    })?;
                    out.push_str(value);
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> TimeZone {
        TimeZone::UTC
    }

    fn vilnius() -> TimeZone {
        TimeZone::get("Europe/Vilnius").unwrap()
    }

    #[test]
    fn parses_literal_and_date_tokens() {
        let template = LayoutTemplate::parse("{date:%Y}/{date:%Y-%m-%d}").unwrap();
        let resolved = template
            .resolve(&HashMap::new(), Timestamp::from_second(0).unwrap(), &utc())
            .unwrap();
        assert_eq!(resolved, "1970/1970-01-01");
    }

    #[test]
    fn bare_date_field_uses_default_format() {
        let template = LayoutTemplate::parse("{date}").unwrap();
        let resolved = template
            .resolve(&HashMap::new(), Timestamp::from_second(0).unwrap(), &utc())
            .unwrap();
        assert_eq!(resolved, "1970-01-01");
    }

    #[test]
    fn resolves_context_field() {
        let template = LayoutTemplate::parse("{event_type}/clip").unwrap();
        let mut context = HashMap::new();
        context.insert("event_type".to_string(), "sentry".to_string());
        let resolved = template
            .resolve(&context, Timestamp::from_second(0).unwrap(), &utc())
            .unwrap();
        assert_eq!(resolved, "sentry/clip");
    }

    #[test]
    fn unknown_field_fails_at_resolution() {
        let template = LayoutTemplate::parse("{missing}").unwrap();
        let err = template
            .resolve(&HashMap::new(), Timestamp::from_second(0).unwrap(), &utc())
            .unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn unclosed_brace_rejected_at_parse() {
        let err = LayoutTemplate::parse("{date:%Y").unwrap_err();
        assert!(err.to_string().contains("unclosed"));
    }

    #[test]
    fn unmatched_closing_brace_rejected_at_parse() {
        let err = LayoutTemplate::parse("stray}brace").unwrap_err();
        assert!(err.to_string().contains("unmatched"));
    }

    #[test]
    fn empty_field_name_rejected_at_parse() {
        let err = LayoutTemplate::parse("{:%Y}").unwrap_err();
        assert!(err.to_string().contains("empty field name"));
    }

    #[test]
    fn invalid_strftime_spec_rejected_at_parse() {
        // A trailing flag with no directive after it is a genuine jiff
        // format-string error (unrecognized specifiers, e.g. %Q, are
        // instead squashed to a literal rather than rejected).
        let err = LayoutTemplate::parse("{date:%-}").unwrap_err();
        assert!(err.to_string().contains("invalid strftime"));
    }

    #[test]
    fn date_renders_in_configured_zone() {
        // Spec scenario: "Layout date renders in the configured zone"
        // 2026-07-04T15:23:51Z in Europe/Vilnius (+03:00) → 18:23:51.
        let ts = Timestamp::from_second(1783178631).unwrap(); // 2026-07-04T15:23:51Z
        let template = LayoutTemplate::parse("{date:%Y-%m-%d}/{date:%H-%M-%S}").unwrap();
        let resolved = template.resolve(&HashMap::new(), ts, &vilnius()).unwrap();
        assert_eq!(resolved, "2026-07-04/18-23-51");
    }

    #[test]
    fn evening_instant_lands_on_local_calendar_day() {
        // Spec scenario: "Evening ride keeps its local calendar day"
        // 2026-07-04T22:30:00Z in America/Los_Angeles (-07:00) → 2026-07-04
        // (not 2026-07-05 which is what UTC would give).
        let ts = Timestamp::from_second(1783204200).unwrap(); // 2026-07-04T22:30:00Z
        let la = TimeZone::get("America/Los_Angeles").unwrap();
        let template = LayoutTemplate::parse("{date:%Y-%m-%d}").unwrap();
        let resolved = template.resolve(&HashMap::new(), ts, &la).unwrap();
        assert_eq!(resolved, "2026-07-04");
    }
}
