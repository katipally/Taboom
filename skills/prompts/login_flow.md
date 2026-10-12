# Login Flow


Use the operator-provisioned vault for credentials, ground the visible fields, verify the filled
form and submit separately when the user's task authorizes login.

## Prepare


1. Check `persona_status` and start a session if needed.
2. Navigate to the login page, wait for the visible transition and take a full screenshot.
3. Ensure the intended browser is focused and not fullscreen.
4. Close unrelated/opaque page tabs and popup windows. Secret entry requires every open page
   host to match the visible allowed HTTPS host; blank/data/file pages also refuse entry.

5. The operator must have added the credential items and unlocked the daemon's vault. If entry
   is refused, describe the reason rather than requesting a password in chat.

## Fill and verify


If username and password fields are both visible with a predictable order, use `batch_actions`
from the latest full `frame_id`. Include described clicks (or a known Tab order) and `type` steps.
Use `<secret>item-name</secret>` for passwords, private usernames and other vault items. A
non-secret username supplied for the task can be typed normally.

The batch returns a fresh screenshot. Verify that the intended fields are filled, without
revealing a masked password. Keep submission clicks/Enter out of the fill batch and submit in a
separate call only after verification.

If login is split across pages, fill/verify the first step, advance with an authorized separate
action, then wait and observe the new step. Do not predict coordinates across that transition.

## Two-factor authentication


After an authorized login submission, wait as needed and inspect the resulting screen.

- For an operator-provisioned TOTP item, focus the observed code field and use
  `type {text: "<secret>totp-name</secret>", mode: "keys"}`. The vault resolves a current code;
  do not delay unnecessarily or ask to reveal the seed.

- For SMS/email, push approval, security-key or physical-device interaction, request manual user
  control with `challenge_handoff`/`view_url` and explain the required action.

- Handoff tools return links immediately. Pause your own actions until user confirmation, then
  observe again; they do not wait or transfer the lease.

Verify the code/form before a separate submission. If another challenge appears, use the
`challenge_flow` prompt.

## After login


Take a fresh screenshot to confirm the actual logged-in state or read the visible error.
Handle optional persistence/save-password prompts according to the user's preferences.
An input call alone does not establish authentication success. Avoid repeatedly retrying a
failed credential or locked account; report the failure.

## Capture behavior


Exact vault typing skips typo planning and clipboard use. Input tracing/saved capture pause during
entry and resume afterward, including saved frames and joined video. Screenshots, zoom, OCR and
visual waits remain available afterward. Visible credentials or codes can appear in responses and
recordings; use masked fields and review artifacts before sharing.

The gate cannot prove that a page input rather than the omnibox has focus. Always ground and focus
the intended field before secret entry. A daemon restart locks the website-typing vault again.
