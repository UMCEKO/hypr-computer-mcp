//! The Anthropic computer-use tool (`computer`, same actions and parameters) as a stdio MCP server driving Hyprland.

mod desktop;
mod pointer;

use std::io::{BufRead as _, Write as _};

use base64::Engine as _;
use serde_json::{Value, json};

use desktop::{Computer, Display, Input, Reply};

const PROTOCOL: &str = "2025-06-18";

fn display() -> Result<Display, String> {
    let max_width = std::env::var("HYPR_COMPUTER_MAX_WIDTH")
        .ok()
        .and_then(|w| w.parse().ok())
        .unwrap_or(1280.0);
    Display::pick(
        std::env::var("HYPR_COMPUTER_MONITOR").ok().as_deref(),
        max_width,
    )
}

fn tool(display: &Display) -> Value {
    let (width, height) = display.size();
    let pair = |what: &str| {
        json!({
            "type": "array",
            "items": { "type": "number" },
            "minItems": 2,
            "maxItems": 2,
            "description": what,
        })
    };
    json!({
        "name": "computer",
        "description": format!(
            "Use a mouse and keyboard to interact with a computer, and take screenshots.\n\
             * The screen is {width}x{height} (monitor {}); every coordinate is in pixels of the screenshots this tool returns.\n\
             * Take a screenshot before acting, and check the one each action answers with before assuming it worked.\n\
             * Click the middle of an element; scroll to bring one into view rather than guessing past the edge.\n\
             * `key` takes xdotool syntax: `Return`, `ctrl+c`, `super+Tab`, several separated by spaces.",
            display.name
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "key", "hold_key", "type", "cursor_position", "mouse_move",
                        "left_mouse_down", "left_mouse_up", "left_click", "left_click_drag",
                        "right_click", "middle_click", "double_click", "triple_click",
                        "scroll", "wait", "screenshot", "zoom"
                    ]
                },
                "coordinate": pair("(x, y): pixels from the screenshot's left and top edges."),
                "start_coordinate": pair("(x, y) where a left_click_drag starts."),
                "text": {
                    "type": "string",
                    "description": "Text to type; the key or chord for key and hold_key; or modifiers to hold during a click or scroll, like `shift` or `ctrl+shift`."
                },
                "scroll_direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                "scroll_amount": { "type": "integer", "minimum": 1, "description": "Wheel notches, 3 by default." },
                "duration": { "type": "number", "description": "Seconds, for hold_key and wait." },
                "region": {
                    "type": "array",
                    "items": { "type": "number" },
                    "minItems": 4,
                    "maxItems": 4,
                    "description": "(x1, y1, x2, y2) to zoom into at full resolution, in screenshot pixels."
                }
            },
            "required": ["action"]
        }
    })
}

fn content(reply: Reply) -> Vec<Value> {
    let mut blocks = Vec::new();
    if let Some(text) = reply.text {
        blocks.push(json!({ "type": "text", "text": text }));
    }
    if let Some(png) = reply.png {
        blocks.push(json!({
            "type": "image",
            "data": base64::engine::general_purpose::STANDARD.encode(png),
            "mimeType": "image/png",
        }));
    }
    blocks
}

fn call(computer: &mut Computer, params: &Value) -> Value {
    if params["name"] != "computer" {
        return json!({
            "content": [{ "type": "text", "text": format!("no tool {}", params["name"]) }],
            "isError": true,
        });
    }
    let outcome = serde_json::from_value::<Input>(params["arguments"].clone())
        .map_err(|e| format!("bad arguments: {e}"))
        .and_then(|input| computer.act(input));
    match outcome {
        Ok(reply) => json!({ "content": content(reply) }),
        Err(error) => json!({ "content": [{ "type": "text", "text": error }], "isError": true }),
    }
}

fn serve(mut computer: Computer) -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(error) => {
                let reply = json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": error.to_string() } });
                writeln!(stdout, "{reply}").map_err(|e| e.to_string())?;
                continue;
            }
        };
        // A notification has no id and wants no answer.
        let Some(id) = message.get("id").cloned() else {
            continue;
        };
        let result = match message["method"].as_str().unwrap_or_default() {
            "initialize" => Ok(json!({
                "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "hypr-computer", "version": env!("CARGO_PKG_VERSION") },
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": [tool(computer.display())] })),
            "tools/call" => Ok(call(&mut computer, &message["params"])),
            other => Err(json!({ "code": -32601, "message": format!("no method {other}") })),
        };
        let reply = match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
        };
        writeln!(stdout, "{reply}").map_err(|e| e.to_string())?;
        stdout.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `call '<arguments json>' [image.png]`: one action from a shell, the screenshot written to a file.
fn call_once(mut computer: Computer, arguments: &str, image: Option<&str>) -> Result<(), String> {
    let input: Input =
        serde_json::from_str(arguments).map_err(|e| format!("bad arguments: {e}"))?;
    let reply = computer.act(input)?;
    if let Some(text) = &reply.text {
        println!("{text}");
    }
    if let Some(png) = &reply.png {
        let path = image.unwrap_or("hypr-computer.png");
        std::fs::write(path, png).map_err(|e| format!("{path}: {e}"))?;
        let (w, h) = computer.display().size();
        println!("{path} ({w}x{h})");
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let outcome =
        display()
            .map(Computer::new)
            .and_then(|computer| match args.get(1).map(String::as_str) {
                Some("call") => call_once(
                    computer,
                    args.get(2)
                        .map(String::as_str)
                        .unwrap_or(r#"{"action":"screenshot"}"#),
                    args.get(3).map(String::as_str),
                ),
                _ => serve(computer),
            });
    if let Err(error) = outcome {
        eprintln!("hypr-computer: {error}");
        std::process::exit(1);
    }
}
