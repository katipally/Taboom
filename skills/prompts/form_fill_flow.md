# Form Fill Flow

How to fill out web forms with Taboom.

## Locate the form

1. Take a screenshot and identify the form and its fields.
2. Use `zoom` on small or dense areas to read field labels clearly.
3. Work top-to-bottom, left-to-right, following the visual order.

## Plan and execute a continuous fill

Use the latest full screenshot's `frame_id` to plan all predictable visible fields. Call
`batch_actions` once with ordered `click`, `type`, and `key` steps, each including its `tool` name.
Use `ctrl+a` to replace existing values and Tab only when the field order is known. Secret
placeholders work in `type` steps. Do not pause for model reasoning between every known field.
The batch returns a fresh screenshot: verify values there, then submit with a separate call.

End a batch before unknown dropdown options, autocomplete suggestions, a page transition, or
validation that changes the layout. If a step fails, use the reported completed steps to resume
from the current state; never replay the whole fill automatically.

## Text inputs

1. Click the field to focus it.
2. If the field has existing text, select all (ctrl+a) and type to replace.
3. Use the `type` tool. For secrets, use `<secret>name</secret>`.
4. Tab to the next field or click it.

## Dropdowns / selects

1. Click the dropdown to open it.
2. Take a screenshot to see the options.
3. Click the correct option. If the list is long, type the first letters to jump.
4. For custom dropdowns (not native selects), the interaction varies:
   - Some open a searchable list: type to filter, then click.
   - Some require scrolling: use `scroll` to find the option.

## Checkboxes and radio buttons

1. Click the checkbox or radio button directly.
2. Screenshot to verify the state changed (checked/unchecked).

## Date pickers

1. If there is a text input, try typing the date directly in the expected format.
2. If a calendar widget opens, navigate month/year first, then click the day.
3. Take a screenshot after each navigation step in the calendar.

## File uploads

1. Locate the file input or "Choose file" button.
2. Click it. A file dialog will open.
3. Use the `type` tool to enter the file path in the dialog, then press Enter.

## Multi-step forms

1. Complete each page fully before clicking Next/Continue.
2. Screenshot after each page transition to confirm the next step loaded.
3. If a progress indicator is visible, note which step you are on.

## Submitting

1. Before submitting, screenshot and verify all visible field values.
2. Click the submit button.
3. Wait for the response and screenshot to confirm success or read errors.
4. If there are validation errors, read each one, fix the field, and re-submit.

## Tips

- Let the humanizer pace keystrokes and pointer motion. Add explicit waits only when the UI needs time.
- If a field has auto-complete suggestions, take a screenshot to check before continuing.
- Required fields are often marked with * or colored borders. Check for these.
- If a form has a CAPTCHA, handle it with the `challenge_flow` prompt before submitting.
