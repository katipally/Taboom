# google.com Playbook

Applies to: `google.com`, `accounts.google.com`, `mail.google.com`

## Cookie consent (EU/EEA visitors)

Google shows a consent dialog before the search page loads.
- Look for "Reject all" button, click it
- If only "I agree" or "Accept all", click "Reject all" if present; otherwise accept

## Search

1. The search input is centered on the page at roughly (960, 500) on a 1920x1080 screen
2. Click the search box (or just start typing; Google auto-focuses)
3. Type the query
4. Press Enter (do not click "Google Search" button; Enter is faster)
5. Wait for results page to settle

## Search results page

- Organic results start below the search bar
- Each result has a title link (blue), URL (green/gray), and snippet
- "People also ask" sections are expandable; click the question to expand
- Pagination is at the bottom; prefer refining the query over paging

## Google Sign-In (accounts.google.com)

1. Email field appears first; type the email, press Enter
2. Wait for the password field to appear (animated transition, ~500ms)
3. Use `SecretTypeRequest` for the password
4. If 2FA is required, wait for the prompt, then use TOTP `SecretTypeRequest`
5. After login, Google may show a "stay signed in" prompt; click "No thanks" or close

## Gmail (mail.google.com)

- Compose: click the "Compose" button (top-left, below the Gmail logo)
- The compose window appears in the bottom-right corner
- To field auto-completes; type the address and press Tab or Enter
- Subject field is next; type and Tab to body
- Body is a rich text editor; plain typing works
