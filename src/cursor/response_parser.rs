use serde::Deserialize;

use super::CursorError;

#[derive(Debug, Deserialize)]
struct CursorEvent {
    #[serde(rename = "type")]
    event_type: Option<String>,
    #[serde(default)]
    is_error: bool,
    result: Option<String>,
}

/// Extrae el último evento `result` de JSON único o JSON por líneas.
pub fn parse_cursor_result(output: &[u8]) -> Result<String, CursorError> {
    let text = std::str::from_utf8(output).map_err(|_| CursorError::InvalidUtf8)?;
    let events = parse_events(text)?;
    let event = events
        .into_iter()
        .rev()
        .find(|event| event.event_type.as_deref() == Some("result"))
        .ok_or(CursorError::MissingResult)?;
    let result = event.result.unwrap_or_default();
    if event.is_error {
        return Err(CursorError::ResultError { message: result });
    }
    let result = result.trim().to_owned();
    if result.is_empty() {
        Err(CursorError::EmptyResult)
    } else {
        Ok(result)
    }
}

fn parse_events(text: &str) -> Result<Vec<CursorEvent>, CursorError> {
    match serde_json::from_str::<CursorEvent>(text) {
        Ok(event) => Ok(vec![event]),
        Err(single_error) => {
            let mut events = Vec::new();
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                match serde_json::from_str::<CursorEvent>(line) {
                    Ok(event) => events.push(event),
                    Err(_) => return Err(CursorError::InvalidJson(single_error)),
                }
            }
            if events.is_empty() {
                Err(CursorError::InvalidJson(single_error))
            } else {
                Ok(events)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_cursor_result;

    #[test]
    fn parses_single_result_and_ignores_unknown_fields() {
        let output = r#"{"type":"result","is_error":false,"result":"feat: añade tabs","extra":1}"#;

        assert_eq!(
            parse_cursor_result(output.as_bytes()).expect("debe interpretar la respuesta"),
            "feat: añade tabs"
        );
    }

    #[test]
    fn finds_result_in_json_lines() {
        let output = concat!(
            "{\"type\":\"system\",\"message\":\"inicio\"}\n",
            "{\"type\":\"result\",\"is_error\":false,\"result\":\"fix: corrige parser\"}\n",
        );

        assert_eq!(
            parse_cursor_result(output.as_bytes()).expect("debe interpretar la respuesta"),
            "fix: corrige parser"
        );
    }

    #[test]
    fn propagates_result_errors() {
        let output = br#"{"type":"result","is_error":true,"result":"policy disabled"}"#;

        assert!(parse_cursor_result(output).is_err());
    }
}
