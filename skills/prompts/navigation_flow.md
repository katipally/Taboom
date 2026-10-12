# Navigation Flow


How to navigate between pages and manage browser state.

## Going to a URL


1. Use `open_url` with the full URL. It opens a new tab using keyboard input.
2. Wait for the page to settle with `wait { until_settled: true }`.
3. Take a screenshot to confirm the page loaded.

## Clicking links


1. Locate the link in a fresh screenshot.
2. Click it. Links may open in the same tab or a new tab.
3. If a new tab opened, inspect whether it became active; switch to it with visible/keyboard input if needed.
4. Screenshot to verify the destination.

## Going back


1. Use `key` with "alt+Left" to go back, or click the browser back button.
2. Screenshot to confirm you returned to the expected page.

## Managing tabs


- `open_url` always opens a new tab. Close old tabs if they accumulate.
- To close the current tab: `key` with "ctrl+w".
- To switch tabs: `key` with "ctrl+Tab" (next) or "ctrl+shift+Tab" (previous).
- Before typing secrets, close unrelated and opaque page tabs/windows. All open page hosts must
  match the visible allowed HTTPS host; blank/data/file pages also refuse entry.

## Handling popups and new windows


1. If a popup blocks the page, dismiss it first (click X, Escape, or the dismiss button).
2. If a new window opened, it may appear as a separate Sway window.
3. Use super+arrows to switch focus between windows if needed.
4. Close popup windows with super+shift+q after handling them.

## Redirects


1. After a click or form submission, the page may redirect.
2. Wait for the redirect to complete with `wait { until_settled: true }`.
3. Screenshot to verify the final destination.
4. If the redirect lands on a login page, follow the `login_flow` prompt.

## Downloads


1. Downloads go to `/home/taboom/Downloads` by default.
2. Take a screenshot to check for a download bar or confirmation.
3. Use `files_list` and `files_get` to inspect/retrieve it. This folder is outside the named data
   volume by default, so retrieve files before container recreation.

## Page state assessment


After any navigation, classify the page state:

- **Ready**: The page loaded and you can proceed with the task.
- **Login required**: A login form is visible. Use `login_flow`.
- **Challenge**: A CAPTCHA or verification is blocking. Use `challenge_flow`.
- **Error**: An error page (404, 500, etc.). Try reloading or a different approach.
- **Consent**: A cookie/consent banner is blocking. Handle it first.
