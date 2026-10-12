# Challenge / CAPTCHA Handling Flow

You have encountered a verification challenge. Follow this decision tree to handle it.

## Step 1: Detect

Call `detect_challenge` (pass `region` if the challenge is in a known area of the screen).
It returns a `challenge_type` and a `suggestion`.

## Step 2: Act by type

### checkbox
A "I'm not a robot" checkbox (reCAPTCHA v2, hCaptcha, GeeTest OneTap).
1. Take a screenshot and locate the checkbox center.
2. Call `challenge_click_hold` at that point. Default hold (800-2000ms) is fine.
3. Screenshot to check: if an image grid or follow-up appeared, re-detect.

### press_hold
A button you must press and hold for 1-3 seconds (HUMAN/PerimeterX, MTCaptcha).
1. Locate the button center.
2. Call `challenge_click_hold` with `hold_ms_min: 1500, hold_ms_max: 3000`.
3. Some require holding until a progress indicator fills. Use the higher range.

### slider
A drag-to-verify slider (GeeTest, DataDome, custom).
1. Locate the slider handle (the draggable element, usually on the left).
2. Locate the target (the gap or destination, usually on the right).
3. Call `challenge_slider` with `from` (handle center) and `to` (target center).
4. The tool overshoots then corrects automatically, mimicking human drag behavior.

### image_grid
A grid of images where you select all matching ones (reCAPTCHA, hCaptcha).
1. Screenshot and identify every tile that matches the prompt.
2. Call `challenge_select` with all matching tile centers as `targets`.
3. If a "Verify" button is separate, click it after selecting tiles.
4. New tiles may load to replace selected ones. Screenshot and repeat if needed.
5. Some hCaptcha rounds have zero matches. If nothing matches, click "Skip."
6. If you cannot identify the correct tiles, call `challenge_handoff`.

### counting
A prompt asking "how many X are in this image?" with multiple-choice answers (hCaptcha).
1. Use `zoom` on the image to count objects carefully.
2. Call `challenge_select` on the correct answer option.

### icon_select
Click icons in a specific order or match shadows (GeeTest, hCaptcha variants).
1. Read the instruction text to determine the required order.
2. Use `zoom` on the instruction area if the text is small.
3. Call `challenge_select` with the icon centers in the exact order shown.

### letter_place
Drag letters or characters into position (hCaptcha variant).
1. This requires drag-and-drop precision beyond current tools.
2. Call `challenge_handoff` with the reason.

### drag_place
Drag an element to a specific 2D position (hCaptcha, GeeTest free-drag).
1. Unlike sliders (1D horizontal), these are 2D free-drag challenges.
2. Call `challenge_handoff` with the reason.

### puzzle
3D rotation, jigsaw, match-three, gobang, dice matching, tile ordering
(FunCaptcha/Arkose, GeeTest). Over 1,250 puzzle variants exist.
1. These require visual reasoning about 3D orientation or spatial fit.
2. Call `challenge_handoff` with the reason.

### audio
Listen to distorted speech and type what you hear (reCAPTCHA, hCaptcha accessibility).
1. Audio challenges require listening capability.
2. Call `challenge_handoff` with the reason.

### text
Type distorted characters shown in an image (legacy CAPTCHAs).
1. Use `zoom` on the CAPTCHA image area to get a clear view.
2. Read the characters from the zoomed image.
3. Locate the text input field and click it.
4. Use the `type` tool to enter the characters.
5. Submit if there is a submit button.

### turnstile
Cloudflare Turnstile (usually invisible/automatic).
1. Wait 3-5 seconds. Turnstile often resolves without interaction.
2. Screenshot to check. If a checkbox appeared, use `challenge_click_hold`.
3. If it shows "checking your browser", wait longer (up to 10 seconds).

### proof_of_work
Browser-side computation (Friendly Captcha, Turnstile PoW, ALTCHA, Kasada, Prosopo).
1. These run automatically in the browser. Wait 5-10 seconds.
2. Screenshot to confirm the challenge resolved.
3. If it stalls, the page may need a reload.

### device_check
Automatic device integrity check (DataDome Device Check, Akamai interstitial).
1. A brief "checking your browser" page appears for 1-5 seconds.
2. Wait for it to complete. Take a screenshot to verify.
3. If it fails, a visible challenge (slider, CAPTCHA) may appear. Re-detect.

### qr_code
QR code scan challenge (reCAPTCHA 2026 cross-device verification).
1. This requires a physical phone to scan the QR code on screen.
2. Call `challenge_handoff` with the reason.

### behavioral (no detect_challenge output; invisible)
Invisible challenges that score your behavior (reCAPTCHA v3, hCaptcha Enterprise).
1. These have no visible UI. If you were flagged, a fallback challenge appears.
2. Detect and handle the fallback challenge.
3. Natural mouse movement, varied timing, and idle pauses reduce flagging.

## Step 3: Verify

After every challenge action, take a screenshot to confirm:
- The challenge widget shows a green check or success state.
- The page has progressed past the challenge.
- No new challenge appeared (some sites chain challenges).

If the challenge failed, try once more. After two failures, call `challenge_handoff`.

## Anti-detection notes

Taboom's humanizer already handles most behavioral signals:
- Mouse curves follow Fitts' law with natural overshoot and tremor.
- Click timing uses lognormal distributions, not fixed delays.
- Idle drift occurs between actions.
- Each persona has a stable, unique style seeded from its identity.

Anti-bot systems collect signals at three layers:
1. **Network**: TLS fingerprint (JA3/JA4), HTTP/2 settings, IP reputation, geo-timezone match.
   Taboom's persona and route handle these.
2. **Environment**: navigator.webdriver, canvas/WebGL fingerprint, fonts, plugins, device memory.
   Taboom's persona configures these through Chrome flags and profile.
3. **Behavioral**: mouse paths, click timing, scroll patterns, idle time, form fill speed.
   Taboom's humanizer generates natural behavioral signals.

For best results:
- Do not rush. Add `wait` calls between actions (1-3 seconds).
- Avoid pixel-perfect clicks. The humanizer adds natural scatter.
- Do not repeat the exact same action immediately after a failure.
- Complete the page task naturally after solving a challenge.
- Vary your timing. Do not click at uniform intervals.
- Include idle pauses where a human would read or think.
