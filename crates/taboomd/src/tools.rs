use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum ToolCall {
    Screenshot {
        #[serde(default)]
        frame_id: Option<String>,
        #[serde(default)]
        max_edge: Option<u32>,
        #[serde(default)]
        region: Option<ToolRegion>,
    },
    Zoom {
        region: ToolRegion,
    },
    CursorPosition,
    Click {
        x: i32,
        y: i32,
        #[serde(default = "default_button")]
        button: String,
        #[serde(default = "default_count")]
        count: u32,
        describe: String,
        #[serde(default)]
        frame_id: Option<String>,
    },
    Move {
        x: i32,
        y: i32,
        #[serde(default)]
        describe: Option<String>,
    },
    Drag {
        from: Point,
        to: Point,
        describe: String,
    },
    Scroll {
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
        direction: String,
        amount: i32,
    },
    Type {
        text: String,
        #[serde(default = "default_mode")]
        mode: String,
        #[serde(default)]
        submit: bool,
    },
    Key {
        combo: String,
    },
    MouseDown {
        #[serde(default = "default_button")]
        button: String,
        #[serde(default)]
        x: Option<i32>,
        #[serde(default)]
        y: Option<i32>,
    },
    MouseUp {
        #[serde(default = "default_button")]
        button: String,
    },
    KeyDown {
        key: String,
    },
    KeyUp {
        key: String,
    },
    HoldKey {
        key: String,
        #[serde(default)]
        duration_ms: Option<u64>,
    },
    Wait {
        #[serde(default)]
        ms: Option<u64>,
        #[serde(default)]
        until_settled: Option<bool>,
        #[serde(default)]
        until_text: Option<String>,
    },
    FindText {
        text: String,
        #[serde(default)]
        region: Option<ToolRegion>,
    },
    ReadText {
        #[serde(default)]
        region: Option<ToolRegion>,
    },
    OpenUrl {
        url: String,
    },
    FilesPut {
        name: String,
        data_base64: String,
    },
    FilesGet {
        path: String,
    },
    FilesList,
    ClipboardGet,
    ClipboardSet {
        text: String,
    },
    SessionStart,
    SessionEnd,
    PersonaStatus,
    ViewUrl,
    RecordingList {
        #[serde(default)]
        persona: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },
    RecordingGet {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        step: Option<u32>,
        #[serde(default)]
        from_step: Option<u32>,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        frames: bool,
    },
    RecordingShare {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        expires_in_s: Option<u64>,
    },
    Computer {
        action: String,
        #[serde(default)]
        coordinate: Option<Vec<i32>>,
        #[serde(default)]
        text: Option<String>,
        #[serde(flatten)]
        extra: ComputerExtra,
    },
}

/// Fields of Anthropic's computer-use schema beyond action/coordinate/text.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComputerExtra {
    #[serde(default)]
    pub start_coordinate: Option<Vec<i32>>,
    #[serde(default)]
    pub scroll_direction: Option<String>,
    #[serde(default)]
    pub scroll_amount: Option<i32>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub region: Option<Vec<i32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRegion {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

fn default_button() -> String {
    "left".into()
}
fn default_count() -> u32 {
    1
}
fn default_mode() -> String {
    "auto".into()
}
pub fn translate_computer_tool(call: &ToolCall) -> Option<ToolCall> {
    match call {
        ToolCall::Computer {
            action,
            coordinate,
            text,
            extra,
        } => {
            match action.as_str() {
                "screenshot" => Some(ToolCall::Screenshot {
                    frame_id: None,
                    max_edge: None,
                    region: None,
                }),
                "click" | "left_click" => {
                    let (x, y) = coords(coordinate)?;
                    Some(ToolCall::Click {
                        x,
                        y,
                        button: "left".into(),
                        count: 1,
                        describe: "computer-use click".into(),
                        frame_id: None,
                    })
                }
                "right_click" => {
                    let (x, y) = coords(coordinate)?;
                    Some(ToolCall::Click {
                        x,
                        y,
                        button: "right".into(),
                        count: 1,
                        describe: "computer-use right click".into(),
                        frame_id: None,
                    })
                }
                "middle_click" => {
                    let (x, y) = coords(coordinate)?;
                    Some(ToolCall::Click {
                        x,
                        y,
                        button: "middle".into(),
                        count: 1,
                        describe: "computer-use middle click".into(),
                        frame_id: None,
                    })
                }
                "double_click" => {
                    let (x, y) = coords(coordinate)?;
                    Some(ToolCall::Click {
                        x,
                        y,
                        button: "left".into(),
                        count: 2,
                        describe: "computer-use double click".into(),
                        frame_id: None,
                    })
                }
                "triple_click" => {
                    let (x, y) = coords(coordinate)?;
                    Some(ToolCall::Click {
                        x,
                        y,
                        button: "left".into(),
                        count: 3,
                        describe: "computer-use triple click".into(),
                        frame_id: None,
                    })
                }
                "mouse_move" => {
                    let (x, y) = coords(coordinate)?;
                    Some(ToolCall::Move {
                        x,
                        y,
                        describe: None,
                    })
                }
                "type" => Some(ToolCall::Type {
                    text: text.clone().unwrap_or_default(),
                    mode: "auto".into(),
                    submit: false,
                }),
                "key" => Some(ToolCall::Key {
                    combo: text.clone().unwrap_or_default(),
                }),
                "cursor_position" => Some(ToolCall::CursorPosition),
                "scroll_up" | "scroll_down" | "scroll" => {
                    let direction = match action.as_str() {
                        "scroll_up" => "up".to_string(),
                        "scroll_down" => "down".to_string(),
                        _ => extra.scroll_direction.clone().unwrap_or_else(|| "down".into()),
                    };
                    let at = coords(coordinate);
                    Some(ToolCall::Scroll {
                        x: at.map(|c| c.0),
                        y: at.map(|c| c.1),
                        direction,
                        amount: extra.scroll_amount.unwrap_or(3),
                    })
                }
                "left_click_drag" => {
                    let (fx, fy) = coords(&extra.start_coordinate)?;
                    let (tx, ty) = coords(coordinate)?;
                    Some(ToolCall::Drag {
                        from: Point { x: fx, y: fy },
                        to: Point { x: tx, y: ty },
                        describe: "computer-use drag".into(),
                    })
                }
                "left_mouse_down" => {
                    let at = coords(coordinate);
                    Some(ToolCall::MouseDown {
                        button: "left".into(),
                        x: at.map(|c| c.0),
                        y: at.map(|c| c.1),
                    })
                }
                "left_mouse_up" => Some(ToolCall::MouseUp { button: "left".into() }),
                "hold_key" => Some(ToolCall::HoldKey {
                    key: text.clone()?,
                    duration_ms: Some((extra.duration.unwrap_or(1.0) * 1000.0) as u64),
                }),
                "wait" => Some(ToolCall::Wait {
                    ms: Some((extra.duration.unwrap_or(1.0) * 1000.0) as u64),
                    until_settled: None,
                    until_text: None,
                }),
                "zoom" => {
                    let r = extra.region.as_ref().filter(|r| r.len() == 4)?;
                    let clamp = |v: i32| v.max(0) as u32;
                    Some(ToolCall::Zoom {
                        region: ToolRegion {
                            x: clamp(r[0]),
                            y: clamp(r[1]),
                            w: clamp(r[2] - r[0]),
                            h: clamp(r[3] - r[1]),
                        },
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn coords(coordinate: &Option<Vec<i32>>) -> Option<(i32, i32)> {
    coordinate.as_ref().and_then(|c| {
        if c.len() >= 2 {
            Some((c[0], c[1]))
        } else {
            None
        }
    })
}

pub fn all_tool_defs() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "screenshot".into(),
            description: "Capture the screen. The image is scaled so its longest edge is at most max_edge (default 1280). All x/y you pass to click, move, drag, scroll and zoom are in the pixel space of the latest full screenshot; Taboom maps them to the real screen. A region screenshot is for looking only and does not change that space. After vault secret typing, screenshots stay disabled for the persistent data volume.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "frame_id": { "type": "string", "description": "Unused on capture; returned ids go to click.frame_id" },
                    "max_edge": { "type": "number", "description": "Longest edge of the returned image, 200-4096 (default 1280)" },
                    "region": {
                        "type": "object",
                        "properties": {
                            "x": { "type": "number" },
                            "y": { "type": "number" },
                            "w": { "type": "number" },
                            "h": { "type": "number" }
                        },
                        "required": ["x", "y", "w", "h"]
                    }
                }
            }),
        },
        ToolDef {
            name: "zoom".into(),
            description: "Crop a region (in latest-screenshot coordinates) and return it at full native resolution, to read small text. Disabled after vault secret typing for the rest of the persistent data volume.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "region": {
                        "type": "object",
                        "properties": {
                            "x": { "type": "number" },
                            "y": { "type": "number" },
                            "w": { "type": "number" },
                            "h": { "type": "number" }
                        },
                        "required": ["x", "y", "w", "h"]
                    }
                },
                "required": ["region"]
            }),
        },
        ToolDef {
            name: "find_text".into(),
            description: "Find visible text with local OCR. Returns matching OCR lines and their boxes in the latest full screenshot's pixel space. An optional region uses those same coordinates. Disabled after vault secret typing for the rest of the persistent data volume.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Case-insensitive text or phrase to find" },
                    "region": {
                        "type": "object",
                        "properties": {
                            "x": { "type": "number" },
                            "y": { "type": "number" },
                            "w": { "type": "number" },
                            "h": { "type": "number" }
                        },
                        "required": ["x", "y", "w", "h"]
                    }
                },
                "required": ["text"]
            }),
        },
        ToolDef {
            name: "read_text".into(),
            description: "Read visible text with local OCR. Returns lines and their boxes in the latest full screenshot's pixel space. An optional region uses those same coordinates. Disabled after vault secret typing for the rest of the persistent data volume.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "region": {
                        "type": "object",
                        "properties": {
                            "x": { "type": "number" },
                            "y": { "type": "number" },
                            "w": { "type": "number" },
                            "h": { "type": "number" }
                        },
                        "required": ["x", "y", "w", "h"]
                    }
                }
            }),
        },
        ToolDef {
            name: "cursor_position".into(),
            description: "Get current cursor position".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "click".into(),
            description: "Click at screen coordinates".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "x": { "type": "number" },
                    "y": { "type": "number" },
                    "button": { "type": "string", "enum": ["left", "right", "middle"] },
                    "count": { "type": "number" },
                    "describe": { "type": "string" },
                    "frame_id": { "type": "string" }
                },
                "required": ["x", "y", "describe"]
            }),
        },
        ToolDef {
            name: "move".into(),
            description: "Move cursor to coordinates".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "x": { "type": "number" },
                    "y": { "type": "number" },
                    "describe": { "type": "string" }
                },
                "required": ["x", "y"]
            }),
        },
        ToolDef {
            name: "drag".into(),
            description: "Drag from one point to another".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "from": {
                        "type": "object",
                        "properties": { "x": { "type": "number" }, "y": { "type": "number" } },
                        "required": ["x", "y"]
                    },
                    "to": {
                        "type": "object",
                        "properties": { "x": { "type": "number" }, "y": { "type": "number" } },
                        "required": ["x", "y"]
                    },
                    "describe": { "type": "string" }
                },
                "required": ["from", "to", "describe"]
            }),
        },
        ToolDef {
            name: "scroll".into(),
            description: "Scroll with the mouse wheel at x,y (default: screen center). amount is wheel notches".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "x": { "type": "number" },
                    "y": { "type": "number" },
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                    "amount": { "type": "number" }
                },
                "required": ["direction", "amount"]
            }),
        },
        ToolDef {
            name: "type".into(),
            description: "Type text into the focused element. mode: keys (real key presses), paste (clipboard + ctrl+v), auto (paste only for very long text). `<secret>NAME</secret>` resolves an unlocked, domain-allowed vault item and types it through the layout-aware keymap with no typo planner or clipboard. It requires a focused, non-fullscreen Chrome window and a healthy route. The private Chrome pipe runs Target.getTargets only and never attaches to a page; local OCR must read a confident hostname from the visible omnibox that matches an HTTPS page-host candidate. HTTP pages are refused, and a hidden scheme with same-host HTTP and HTTPS candidates is ambiguous and refused. Taboom cannot verify that the HTML input rather than the address bar has focus; click the intended page field and keep the page stable. After secret typing, screen images and video are disabled for the persistent data volume. `submit` presses Enter after typing; secret calls refuse paste mode.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "mode": { "type": "string", "enum": ["auto", "keys", "paste"] },
                    "submit": { "type": "boolean" }
                },
                "required": ["text"]
            }),
        },
        ToolDef {
            name: "key".into(),
            description: "Press keys on the keyboard: a combo like ctrl+c, alt+Tab, Enter, F5, or a space-separated sequence like \"ctrl+a Delete\". Desktop shortcuts: super+b browser (focus it, or reopen it if closed), super+Return terminal, super+d app launcher, super+shift+q close window, super+f fullscreen, super+arrows move focus, super+1..4 workspaces, super+w tabbed layout. The top bar also has clickable Browser, Terminal and Apps buttons.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "combo": { "type": "string" }
                },
                "required": ["combo"]
            }),
        },
        ToolDef {
            name: "mouse_down".into(),
            description: "Press and hold a mouse button, optionally after moving to x,y. Stays held until mouse_up (for custom drags, drawing, selecting)".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "button": { "type": "string", "enum": ["left", "right", "middle"] },
                    "x": { "type": "number" },
                    "y": { "type": "number" }
                }
            }),
        },
        ToolDef {
            name: "mouse_up".into(),
            description: "Release a held mouse button".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "button": { "type": "string", "enum": ["left", "right", "middle"] }
                }
            }),
        },
        ToolDef {
            name: "key_down".into(),
            description: "Press and hold one key (e.g. shift, ctrl, a) until key_up. Use for shift+click range selects and the like".into(),
            input_schema: json!({
                "type": "object",
                "properties": { "key": { "type": "string" } },
                "required": ["key"]
            }),
        },
        ToolDef {
            name: "key_up".into(),
            description: "Release a key held with key_down".into(),
            input_schema: json!({
                "type": "object",
                "properties": { "key": { "type": "string" } },
                "required": ["key"]
            }),
        },
        ToolDef {
            name: "hold_key".into(),
            description: "Hold one key for duration_ms (default 1000, max 10000), then release".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string" },
                    "duration_ms": { "type": "number" }
                },
                "required": ["key"]
            }),
        },
        ToolDef {
            name: "wait".into(),
            description: "Wait ms milliseconds, until the screen stops changing, or until local OCR finds until_text. For until_settled and until_text, ms is the cap (default 3000). Screen OCR is disabled after vault secret typing for the rest of the persistent data volume.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "ms": { "type": "number", "description": "0-60000" },
                    "until_settled": { "type": "boolean" },
                    "until_text": { "type": "string", "description": "Wait until this visible text appears; case-insensitive" }
                }
            }),
        },
        ToolDef {
            name: "open_url".into(),
            description: "Open a URL in a new browser tab, done with the keyboard: super+b (brings the browser back if it was closed), ctrl+t, type the address, Enter".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" }
                },
                "required": ["url"]
            }),
        },
        ToolDef {
            name: "files_put".into(),
            description: "Upload a file via file dialog".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "data_base64": { "type": "string" }
                },
                "required": ["name", "data_base64"]
            }),
        },
        ToolDef {
            name: "files_get".into(),
            description: "Download a file by path".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "files_list".into(),
            description: "List downloaded files".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "clipboard_get".into(),
            description: "Get clipboard contents".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "clipboard_set".into(),
            description: "Set clipboard contents".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" }
                },
                "required": ["text"]
            }),
        },
        ToolDef {
            name: "session_start".into(),
            description: "Start a session on this container's desktop: leases it to you alone, starts the recording and loads the persona's keyboard layout. Refused while another agent holds it or the route check is failing".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "session_end".into(),
            description: "End your session: releases held keys and buttons, stops the recording and frees the desktop".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "persona_status".into(),
            description: "Which persona this container is, its declared vs. applied settings, route health (exit IP, country, ASN) and the current session. Works without a session".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "view_url".into(),
            description: "Get the live view URL for this container's desktop".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "recording_list".into(),
            description: "List recorded agent sessions, newest first. Every session (session_start to session_end) records each tool call and a frame after each action".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "persona": { "type": "string" },
                    "limit": { "type": "number", "description": "default 20" }
                }
            }),
        },
        ToolDef {
            name: "recording_get".into(),
            description: "Read a recording (default: your current session). With step: that step plus its frame image. Otherwise a page of steps from from_step; frames=true also returns their images (max 10 per call)".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "step": { "type": "number" },
                    "from_step": { "type": "number" },
                    "limit": { "type": "number", "description": "default 50" },
                    "frames": { "type": "boolean" }
                }
            }),
        },
        ToolDef {
            name: "recording_share".into(),
            description: "Make a signed, expiring link to a web replay page of a recording (default: your current session) that a person can open in a browser".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "expires_in_s": { "type": "number", "description": "default 604800 (7 days)" }
                }
            }),
        },
        ToolDef {
            name: "computer".into(),
            description: "Anthropic computer-use compatible tool (translated to native tools)".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": [
                        "screenshot", "click", "left_click", "right_click",
                        "middle_click", "double_click", "triple_click",
                        "mouse_move", "left_click_drag", "left_mouse_down", "left_mouse_up",
                        "type", "key", "hold_key", "cursor_position", "scroll",
                        "scroll_up", "scroll_down", "wait", "zoom"
                    ]},
                    "coordinate": { "type": "array", "items": { "type": "number" } },
                    "start_coordinate": { "type": "array", "items": { "type": "number" } },
                    "text": { "type": "string" },
                    "scroll_direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                    "scroll_amount": { "type": "number" },
                    "duration": { "type": "number", "description": "seconds, for wait and hold_key" },
                    "region": { "type": "array", "items": { "type": "number" }, "description": "[x0, y0, x1, y1], for zoom" }
                },
                "required": ["action"]
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_tool_schemas_have_object_type() {
        for def in all_tool_defs() {
            assert_eq!(
                def.input_schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "tool '{}' schema must be type:object",
                def.name,
            );
        }
    }

    #[test]
    fn no_top_level_oneof_anyof() {
        for def in all_tool_defs() {
            assert!(
                def.input_schema.get("oneOf").is_none(),
                "tool '{}' has top-level oneOf",
                def.name,
            );
            assert!(
                def.input_schema.get("anyOf").is_none(),
                "tool '{}' has top-level anyOf",
                def.name,
            );
        }
    }

    #[test]
    fn translate_computer_screenshot() {
        let call = ToolCall::Computer {
            action: "screenshot".into(),
            coordinate: None,
            text: None,
            extra: Default::default(),
        };
        let translated = translate_computer_tool(&call).unwrap();
        assert!(matches!(translated, ToolCall::Screenshot { .. }));
    }

    #[test]
    fn translate_computer_click() {
        let call = ToolCall::Computer {
            action: "left_click".into(),
            coordinate: Some(vec![100, 200]),
            text: None,
            extra: Default::default(),
        };
        let translated = translate_computer_tool(&call).unwrap();
        match translated {
            ToolCall::Click { x, y, button, .. } => {
                assert_eq!(x, 100);
                assert_eq!(y, 200);
                assert_eq!(button, "left");
            }
            _ => panic!("expected Click"),
        }
    }

    #[test]
    fn translate_computer_double_click() {
        let call = ToolCall::Computer {
            action: "double_click".into(),
            coordinate: Some(vec![50, 75]),
            text: None,
            extra: Default::default(),
        };
        let translated = translate_computer_tool(&call).unwrap();
        match translated {
            ToolCall::Click { count, .. } => assert_eq!(count, 2),
            _ => panic!("expected Click"),
        }
    }

    #[test]
    fn translate_computer_type() {
        let call = ToolCall::Computer {
            action: "type".into(),
            coordinate: None,
            text: Some("hello".into()),
            extra: Default::default(),
        };
        let translated = translate_computer_tool(&call).unwrap();
        match translated {
            ToolCall::Type { text, .. } => assert_eq!(text, "hello"),
            _ => panic!("expected Type"),
        }
    }

    #[test]
    fn translate_computer_key() {
        let call = ToolCall::Computer {
            action: "key".into(),
            coordinate: None,
            text: Some("ctrl+c".into()),
            extra: Default::default(),
        };
        let translated = translate_computer_tool(&call).unwrap();
        match translated {
            ToolCall::Key { combo } => assert_eq!(combo, "ctrl+c"),
            _ => panic!("expected Key"),
        }
    }

    #[test]
    fn translate_unknown_action_returns_none() {
        let call = ToolCall::Computer {
            action: "dance".into(),
            coordinate: None,
            text: None,
            extra: Default::default(),
        };
        assert!(translate_computer_tool(&call).is_none());
    }

    #[test]
    fn translate_click_without_coords_returns_none() {
        let call = ToolCall::Computer {
            action: "click".into(),
            coordinate: None,
            text: None,
            extra: Default::default(),
        };
        assert!(translate_computer_tool(&call).is_none());
    }

    #[test]
    fn translate_computer_scroll_and_drag() {
        let scroll = translate_computer_tool(&ToolCall::Computer {
            action: "scroll".into(),
            coordinate: None,
            text: None,
            extra: ComputerExtra {
                scroll_direction: Some("left".into()),
                scroll_amount: Some(5),
                ..Default::default()
            },
        });
        assert!(matches!(
            scroll,
            Some(ToolCall::Scroll { x: None, y: None, ref direction, amount: 5 }) if direction == "left"
        ));

        let drag = translate_computer_tool(&ToolCall::Computer {
            action: "left_click_drag".into(),
            coordinate: Some(vec![30, 40]),
            text: None,
            extra: ComputerExtra { start_coordinate: Some(vec![10, 20]), ..Default::default() },
        });
        match drag {
            Some(ToolCall::Drag { from, to, .. }) => assert_eq!((from.x, from.y, to.x, to.y), (10, 20, 30, 40)),
            other => panic!("expected Drag, got {other:?}"),
        }
    }

    #[test]
    fn computer_extra_fields_deserialize() {
        let call: ToolCall = serde_json::from_value(json!({
            "tool": "computer", "action": "wait", "duration": 1.5
        }))
        .unwrap();
        assert!(matches!(translate_computer_tool(&call), Some(ToolCall::Wait { ms: Some(1500), .. })));
    }

    #[test]
    fn ocr_and_text_wait_calls_deserialize() {
        let find: ToolCall = serde_json::from_value(json!({
            "tool": "find_text",
            "text": "Continue",
            "region": { "x": 10, "y": 20, "w": 300, "h": 100 }
        })).unwrap();
        assert!(matches!(find, ToolCall::FindText { text, region: Some(_) } if text == "Continue"));

        let read: ToolCall = serde_json::from_value(json!({ "tool": "read_text" })).unwrap();
        assert!(matches!(read, ToolCall::ReadText { region: None }));

        let wait: ToolCall = serde_json::from_value(json!({
            "tool": "wait", "until_text": "Page loaded", "ms": 5000
        })).unwrap();
        assert!(matches!(wait, ToolCall::Wait { ms: Some(5000), until_text: Some(ref text), .. } if text == "Page loaded"));
    }

    #[test]
    fn tool_def_names_unique() {
        let defs = all_tool_defs();
        let mut names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        names.sort();
        let len_before = names.len();
        names.dedup();
        assert_eq!(len_before, names.len(), "duplicate tool names found");
    }
}
