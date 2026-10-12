# Input Tools Guide

How to interact with the desktop through mouse and keyboard.

## Mouse tools

### click
Click at screen coordinates with human-realistic timing (Fitts' law curves, natural
overshoot, lognormal hold duration).

**Parameters:**
- `x`, `y`: Coordinates from the latest screenshot.
- `describe`: What you intend to do. "Click the Submit button" not "Click at 450,320".
- `button`: "left" (default), "right", "middle".
- `count`: 1 (default) or 2 for double-click.
- `frame_id`: From the screenshot. If the frame is stale, the tool rejects the click.

**Tips:**
- Click the center of text labels, not element edges. The humanizer adds natural scatter.
- For right-click context menus: `button: "right"`.
- For opening links in new tabs: right-click, then click "Open link in new tab".
- Double-click to select a word in a text field.

### move
Move the cursor without clicking. Use for hovering to trigger tooltips, dropdown reveals,
or to position before a mouse_down.

### drag
Drag from one point to another. Both points are in screenshot coordinates.

**When to use:**
- Moving elements (drag-and-drop).
- Selecting text by dragging.
- Resizing elements by dragging edges/handles.
- Scrollbar dragging.

**Parameters:** `from` ({x, y}), `to` ({x, y}), `describe`.

### scroll
Scroll at a position using the mouse wheel.

**Parameters:**
- `x`, `y`: Where to scroll (default: screen center).
- `direction`: "up", "down", "left", "right".
- `amount`: Number of wheel notches. 3-5 for a casual scroll, 10+ for fast scrolling.

**Tips:**
- Scroll at the center of the content area, not the edge.
- After scrolling, take a screenshot to see the new content.
- For long pages, use larger amounts (10-20 notches) to move faster.
- For precise scrolling (like in a dropdown list), use smaller amounts (1-2 notches).

### mouse_down / mouse_up
Low-level held mouse button. Use when you need the button held across multiple actions
(like custom drag paths, drawing, or range selection with modifier keys).

**Pattern -- shift+click range select:**
```
key_down { key: "shift" }
click { x, y, describe: "Select range endpoint" }
key_up { key: "shift" }
```

**Pattern -- custom drag path:**
```
mouse_down { button: "left", x: 100, y: 200 }
move { x: 300, y: 200 }
move { x: 300, y: 400 }
mouse_up { button: "left" }
```

## Keyboard tools

### type
Type text into the focused element. The humanizer adds realistic keystroke timing,
occasional typos (corrected with backspace), and varied rhythm.

**Modes:**
- `keys` (default): Real key presses through the persona's keyboard layout. Natural typing.
- `paste`: Clipboard paste (ctrl+v). Faster for long text.
- `auto`: Paste only for very long text, keys otherwise.

**Secret typing:** Use `<secret>name</secret>` to type vault credentials. This:
- Resolves the secret from the encrypted vault.
- Types through the keymap with no typos, no clipboard.
- Requires a focused, non-fullscreen Chrome window on an HTTPS page.
- Checks the visible omnibox hostname against the secret's allowed domains.
- Disables all visual capture afterward.

**Parameters:**
- `text`: The text to type. Or `<secret>name</secret>`.
- `mode`: "auto", "keys", or "paste".
- `submit`: If true, presses Enter after typing. Refused for secret calls.

**Tips:**
- Always click the target field before typing.
- To clear a field: `key { combo: "ctrl+a" }` then type the new content.
- For multiline text: include `\n` in the text string.
- Tab between fields: include `\t` or use `key { combo: "Tab" }` separately.

### key
Press a key combination. Supports modifiers (ctrl, alt, shift, super) and sequences.

**Common combos:**
- `ctrl+a` -- Select all
- `ctrl+c` / `ctrl+v` -- Copy / paste
- `ctrl+z` -- Undo
- `ctrl+w` -- Close tab
- `ctrl+t` -- New tab
- `ctrl+l` -- Focus address bar
- `ctrl+Tab` / `ctrl+shift+Tab` -- Next / previous tab
- `Enter` -- Submit / confirm
- `Escape` -- Cancel / close
- `Tab` / `shift+Tab` -- Next / previous field
- `F5` -- Reload page
- `alt+Left` / `alt+Right` -- Back / forward

**Sequences:** Space-separated combos execute in order.
`ctrl+a Delete` selects all then deletes.

**Desktop shortcuts (Sway window manager):**
- `super+b` -- Focus browser (reopens if closed)
- `super+Return` -- Open terminal
- `super+d` -- App launcher
- `super+shift+q` -- Close focused window
- `super+f` -- Toggle fullscreen
- `super+Left/Right/Up/Down` -- Move focus between windows
- `super+1` through `super+4` -- Switch workspace

### key_down / key_up
Hold a key across multiple actions. Use for modifier keys during mouse actions.

### hold_key
Hold one key for a duration then release. Max 10000ms.

## wait
Pause between actions or wait for conditions.

**Modes:**
- Time: `wait { ms: 2000 }` -- Wait 2 seconds.
- Settled: `wait { until_settled: true }` -- Wait until the screen stops changing
  (animations finish, page loads complete). Default timeout 3000ms.
- Text: `wait { until_text: "Success" }` -- Wait until specific text appears on screen.
  Case-insensitive.

**When to use wait:**
- After `open_url`: `wait { until_settled: true }` before screenshotting.
- After clicking a button that triggers loading: `wait { until_settled: true }`.
- After form submission: `wait { until_text: "Thank you" }`.
- Between actions to appear more human: `wait { ms: 500 }`.

## Input ordering best practices

1. **Always click before typing.** The field must have focus.
2. **Screenshot before clicking.** You need accurate coordinates.
3. **Wait after navigation.** Use `until_settled` before taking the next screenshot.
4. **Verify after important actions.** Screenshot to confirm buttons were clicked, forms submitted.
5. **Release what you hold.** If you use mouse_down/key_down, always follow with mouse_up/key_up.
   session_end releases everything, but do not rely on that mid-task.
