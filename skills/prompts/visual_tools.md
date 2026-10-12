# Visual Tools Guide

How to see, read, and understand what is on the screen.

## screenshot

Your primary way of seeing the screen. Returns a PNG image scaled to fit `max_edge`
(default 1280 pixels on the longest side). Every screenshot includes a `frame_id` that
coordinates reference.

**When to take a screenshot:**
- Before any click, drag, or type action (you need fresh coordinates).
- After any action that changes the screen (to verify the result).
- When you are unsure what is on the screen.

**Parameters:**
- `max_edge`: 200-4096. Increase for dense UIs; decrease to save tokens.
- `region`: Crop to a specific area (same coordinate space as previous screenshot).

**Tips:**
- Keep only the latest 2 screenshots in your conversation context. Older ones waste tokens.
- Screenshots are scaled. Your click coordinates should match this scaled image, not the
  native screen resolution. Taboom handles the mapping.

## zoom

Crops a region from the latest screenshot and returns it at full native resolution.
Use it to read small text that is blurry in the scaled screenshot.

**When to use:**
- Button labels or menu items are too small to read.
- Form field contents need verification.
- Error messages or status text are unclear.
- CAPTCHA text needs to be read character by character.

**Important:** zoom does NOT change the coordinate space. After zooming, your next click
still uses coordinates from the full screenshot, not the zoomed image.

## read_text

Runs local OCR (Tesseract) on the full screen or a region. Returns text lines with
bounding boxes in the latest screenshot's coordinate space.

**When to use:**
- You need the exact text content of the screen.
- Faster than screenshot when you only need text, not the image.
- Checking for specific content without taking a screenshot.

**Parameters:**
- `region`: Optional. Narrow OCR to a specific area for faster, more accurate results.

## find_text

Like `read_text` but filters for a specific text string. Case-insensitive. Returns only
matching lines and their bounding boxes.

**When to use:**
- Looking for a specific button, link, or label.
- Checking if a page has loaded by looking for expected text.
- Finding where a specific element is on screen.

**Parameters:**
- `text`: The text to search for (case-insensitive).
- `region`: Optional. Narrow the search area.

**Tips:**
- Use short, distinctive phrases. "Submit" is better than "Please submit your form."
- The bounding box in the result gives you coordinates for clicking.

## cursor_position

Returns the current cursor coordinates. Useful when you need to know where the cursor
is without taking a screenshot.

## Visual capture after secrets

After typing any `<secret>name</secret>` value, screenshots, zoom, read_text, find_text,
and video recording are permanently disabled for the rest of the data volume. Plan your
visual verification steps before typing secrets.

## Efficient visual workflows

1. **Broad then narrow:** Take a full screenshot first. If text is too small, zoom the area.
2. **OCR for data, screenshots for layout:** Use read_text/find_text when you need text
   content. Use screenshots when you need to understand page layout or find visual elements.
3. **Region-scoped OCR:** Pass a region to read_text/find_text when you know where to look.
   Full-screen OCR is slower and returns more noise.
4. **wait + until_text:** Instead of polling with screenshots, use `wait { until_text: "Success" }`
   to wait for specific text to appear.
