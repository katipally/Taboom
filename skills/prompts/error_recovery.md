# Error Recovery

What to do when things go wrong.

## Stale coordinates

**Symptom**: Click hits the wrong target, or the tool returns a "frame is stale" error.
**Fix**: Take a fresh screenshot. Use the new frame's coordinates.

## Element not found

**Symptom**: The button, field, or link you expected is not visible.
**Fix**:
1. Take a fresh screenshot.
2. Scroll down to check if the element is below the fold.
3. If the page looks different, reassess the page state (maybe it navigated).
4. Use `zoom` on the area where you expect the element.

## Page did not load

**Symptom**: Blank page, spinner, or error message after navigation.
**Fix**:
1. Call `wait { until_settled: true }` to give the page time.
2. Screenshot to check the state.
3. If still loading after 10 seconds, try `key` with F5 to reload.
4. If the error persists, check the URL and try `open_url` again.

## Click had no effect

**Symptom**: You clicked a button but nothing happened.
**Fix**:
1. Screenshot to verify the button state.
2. The click may have landed on an overlay, popup, or tooltip. Dismiss it first.
3. Try clicking the exact center of the button text, not the button edge.
4. If the button requires a form to be valid first, check for validation errors.

## Typing went wrong

**Symptom**: Text appeared in the wrong field, was duplicated, or was garbled.
**Fix**:
1. Click the target field to ensure it has focus.
2. Select all (ctrl+a) to clear existing content.
3. Retype. If the field has input masking or formatting, type slower.

## Loop detection

If you have performed the same action 5 times with no change:
1. Stop immediately. You are stuck.
2. Take a screenshot and reassess the page state.
3. Try a completely different approach:
   - Different element to interact with
   - Navigate away and come back
   - Use a different tool (keyboard vs mouse)
4. If still stuck, describe the situation to the user.

## Timeout errors

**Symptom**: A tool call timed out.
**Fix**:
1. The action may still be in progress. Wait and screenshot.
2. If the page crashed, use super+b to reopen the browser.
3. If the browser is unresponsive, the container may need a restart.

## Route failures

**Symptom**: `persona_status` shows an unhealthy route, or tools refuse with a route error.
**Fix**: Stop and inform the user. Do not try to work around route failures.
