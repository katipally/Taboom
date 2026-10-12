# Google Site Playbook


Applies to `google.com`, `accounts.google.com` and `mail.google.com`.
Page layouts and flows vary; use the current screenshot rather than fixed positions, colors or delays.

## Consent


If a consent dialog is visible, read its available choices and follow the user's privacy preference.
Ask when that preference is unclear; do not assume optional tracking is authorized.

## Search


1. Ground the search field in a full screenshot.
2. Focus it, type the query and press Enter.
3. Wait if needed, then inspect the results.
4. Select a visible result relevant to the task, or refine the query if results are unsuitable.
   Expand related questions or use pagination only when they help the assigned task.

## Website sign-in


This refers to website authentication, not browser account sign-in or sync (disabled by policy).

1. Use the `login_flow` prompt and the operator-provisioned vault.
2. Close unrelated/opaque page tabs/windows before secret entry. Every open page host must match
   the visible allowed HTTPS host; observe again after redirects between Google hosts.

3. Fill predictable visible fields in one batch, verify and advance/submit separately when authorized.
4. If the flow splits email/password or shows another challenge, stop the sequence and observe the
   new step rather than predicting its coordinates.

5. Use a configured TOTP placeholder for the observed code field or coordinate manual user control.
6. Handle persistence prompts according to the user's preferences and verify the actual logged-in state.

## Gmail


- Locate Compose in a fresh screenshot and inspect the resulting form.
- Batch predictable visible recipient, subject and body fields in their known order.
- Stop to observe recipient autocomplete choices or other layout changes.
- Verify recipients and content before a separate send action; sending requires task authorization.
- Confirm the visible sent/error state rather than treating a click as proof of delivery.
