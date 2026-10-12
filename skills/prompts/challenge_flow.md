# Challenge / Verification Flow


These tools provide local OCR cues and native input gestures. The external agent supplies visual
reasoning; no helper guarantees that a challenge will be solved or accepted.

## Observe and classify


1. Take a full screenshot and inspect the visible instruction.
2. Optionally call `detect_challenge` with a `region`. It returns `challenge_type`,
   `detected_cues`, `suggestion` and a short OCR text sample.

3. Compare the suggestion to the screenshot before acting. OCR can misclassify; `none` only
   means no supported text cue matched, not proof that no verification is present.

## Choose a grounded action


The detector currently returns the following cue categories:

| Category | Guidance |
|---|---|
| `checkbox` | Locate the checkbox in the image; use a described native click/hold and verify its state. |
| `press_hold` | Use `challenge_click_hold` with the duration required by the visible prompt; defaults are 800–2000 ms. |
| `slider` | Use `challenge_slider` with the observed handle/target points, then inspect the result. |
| `image_grid` | Identify matching tiles visually; `challenge_select` clicks supplied points in order. Observe replacement tiles before another sequence. |
| `counting` | Inspect/zoom the image, count visually and select the observed answer. |
| `icon_select` | Read the required order and pass the observed target points in that order. |
| `text` | Zoom/read the image, focus its input and type the answer if confident. |
| `turnstile` | Wait for the visible automatic check, then inspect whether an action is needed. |
| `proof_of_work` | Give the browser time for its visible computation; observe completion or failure. |
| `device_check` | Wait for the visible check; inspect any resulting challenge or error. |
| `letter_place` | Request manual user control when precise placement cannot be grounded confidently. |
| `drag_place` | Use ordinary drag tools only if the task/coordinates are clear; otherwise request manual control. |
| `puzzle` | Request user help when the required spatial reasoning is beyond the agent's confidence. |
| `audio` | Request user help; Taboom has no audio-listening tool. |
| `qr_code` | Request user control for the physical device interaction. |
| `none` | Continue only after inspecting the actual page state. |

`challenge_click_hold` requires `x`, `y` and `describe`, with optional `frame_id` and
`hold_ms_min`/`hold_ms_max`. `challenge_slider` requires `from`, `to` and `describe`.
`challenge_select` requires `targets` and `describe`, accepts at most 20 points, and may take
`frame_id`. The agent determines the correct targets; these tools perform the supplied gestures.

Do not batch across replacement images, hidden challenge rounds or other unpredictable changes.
A successful input call reports that input was performed, not that the site accepted it.

## Verify and recover


After an action, inspect a fresh screenshot for a success state, page progression or a new
challenge. Wait only when the UI needs time. If an attempt fails, reassess instead of immediately
repeating the same coordinates. After two unsuccessful attempts, request user help.

Human-derived geometry and heuristic input timing are research models. They do not guarantee
detection avoidance, change network TLS fingerprints or configure every browser fingerprint.

## Manual user control


Call `challenge_handoff` with a reason, or call `view_url`. `challenge_handoff` returns
`watch_url` and `takeover_url` immediately; `view_url` uses `url` and `takeover_url`.
These tools do not message the user, wait, release/transfer the lease or prevent simultaneous input.

Tell the user what is blocking progress and provide the control link. Pause your actions until
they confirm completion, then take a full screenshot and continue from the current state.
Manual input remains visible to live view and recording; the vault's exact-entry capture pause
applies to MCP secret entry, not arbitrary user keystrokes.
