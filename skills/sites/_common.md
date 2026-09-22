# Common Site Playbook Patterns

## Before any interaction

1. Take a screenshot to confirm the page loaded
2. Wait for the page to settle (no spinners, no skeleton loaders)
3. Identify the current state: logged in, logged out, CAPTCHA, cookie banner

## Cookie / consent banners

- Look for "Accept", "Accept all", or "Reject all" buttons
- Click "Reject all" when available, otherwise "Accept"
- If no button visible, try scrolling down; some banners appear after scroll

## Login flows

1. Never type credentials until you confirm the domain in the address bar
2. Use `SecretTypeRequest` for passwords and TOTP codes
3. After typing a password, wait 500ms then take a screenshot to verify the field filled
4. For TOTP: request the code, type it immediately (codes expire in 30s)

## Search

1. Click the search input or press `/` (many sites use this shortcut)
2. Type the query using `KeyType`
3. Press Enter
4. Wait for results to settle before reading them

## Navigation

- Prefer clicking visible links over typing URLs
- After any navigation, wait for the page to settle before interacting
- If a page takes more than 5 seconds to load, take a screenshot and report

## Error recovery

- If an element is not found, take a screenshot and re-assess
- If a click lands on the wrong element, take a screenshot to verify coordinates
- If a form submission fails, read any error messages on screen via screenshot
