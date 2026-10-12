# Login Flow

How to log into a website using Taboom.

## Before you start

1. Call `persona_status` to confirm the route is healthy.
2. Call `session_start` if no session is active.
3. Navigate to the login page with `open_url`.
4. Take a screenshot to confirm the page loaded.

## Entering credentials

1. Locate the username/email field in the screenshot. Click it.
2. Use `type` with the credential value. For non-secret values (like a username the user
   provided in chat), type it directly. For passwords, always use `<secret>name</secret>`.
3. If the login is split into steps (email first, then password), press Enter or click
   the "Next" button after the email, then wait for the password field to appear.
4. Take a screenshot after the transition before typing the password.
5. Use `type` with `<secret>password-name</secret>` for the password.

## Two-factor authentication

1. After submitting the password, screenshot to check for a 2FA prompt.
2. For TOTP: use `type` with `<secret>totp-name</secret>` immediately. TOTP codes expire
   in 30 seconds, so do not delay.
3. For SMS/email codes: call `challenge_handoff` since the user needs to provide the code.
4. For push notifications: call `challenge_handoff` and tell the user to approve the push.
5. For security keys: call `challenge_handoff` since physical interaction is required.

## Post-login

1. Take a screenshot to confirm you are logged in.
2. Dismiss any "stay signed in" or "save password" prompts.
3. If a CAPTCHA appears during login, follow the `challenge_flow` prompt.
4. If login failed, read the error message and adjust (wrong credentials, account locked, etc.).

## Important

- Never type a password or secret directly. Always use `<secret>name</secret>`.
- The vault must have the secret pre-loaded by the operator via `taboom vault add`.
- Screenshots, zoom, OCR and visual waits remain available after secret typing. Take a fresh
  screenshot to verify the transition and locate a 2FA field. Visible secrets can appear in live
  responses, so use masked fields and do not reveal their contents. Saved frames and video pause
  during secret entry and resume afterward; review them before sharing.
- Close other tabs and windows before typing secrets (the omnibox host check requires
  all visible tabs to be on the same domain).
