# Browser Interaction Patterns


Common patterns for web browsing tasks. Each pattern shows the tool sequence.

## Open a page and read it


```
session_start
open_url { url: "https://example.com" }
wait { until_settled: true }
screenshot
[read the screenshot, extract information]
session_end
```

## Fill visible form fields in one flow


Observe the form once, then use `batch_actions` with the latest `frame_id` and ordered native
steps (`tool`, plus each tool's arguments). Fill all predictable visible fields, verify the returned
screenshot, and submit separately. Stop before an unknown menu, autocomplete choice or page change.
This avoids a model round trip after every field. See `form_fill_flow` for the full sequence.

## Fill or correct one field


```
screenshot                                    [find the field]
click { x, y, describe: "Click email field" } [focus it]
key { combo: "ctrl+a" }                       [select existing text]
type { text: "user@example.com" }             [type new value]
screenshot                                    [verify]
```

## Select from a dropdown


```
screenshot                                    [find the dropdown]
click { x, y, describe: "Open country dropdown" }
wait { until_settled: true }                  [wait for options to appear]
screenshot                                    [see the options]
[if option visible] click { x, y, describe: "Select United States" }
[if option not visible] type { text: "United" } [type to filter]
wait { ms: 500 }
screenshot                                    [find filtered option]
click { x, y, describe: "Select United States" }
```

## Scroll to find content


```
screenshot                                    [check current view]
[content not visible]
scroll { direction: "down", amount: 5 }
screenshot                                    [check new content]
[repeat until found or bottom reached]
```

## Handle a popup or modal


```
screenshot                                    [see the popup]
[identify dismiss button: X, Close, Cancel, etc.]
click { x, y, describe: "Close cookie consent popup" }
wait { ms: 500 }
screenshot                                    [verify popup is gone]
```

## Download a file


```
click { x, y, describe: "Click download button" }
wait { ms: 3000 }                             [wait for download]
files_list                                    [check downloads]
files_get { path: "file.pdf" }       [retrieve the file]
```

## Upload a file


```
files_put { name: "document.pdf", data_base64: "..." }  [stage the file]
click { x, y, describe: "Click Choose File button" }
wait { until_settled: true }                             [file dialog opens]
screenshot                                               [see the dialog]
type { text: "<path returned by files_put>" }            [use the actual staged path]
key { combo: "Enter" }                                   [confirm selection]
wait { ms: 1000 }
screenshot                                               [verify file selected]
```

## Copy text from a page


```
screenshot                                    [find the text]
click { x, y, describe: "Click start of text" }
key_down { key: "shift" }
click { x: x2, y: y2, describe: "Select to end of text" }
key_up { key: "shift" }
key { combo: "ctrl+c" }
clipboard_get                                 [read the copied text]
```

## Handle a slow page


```
open_url { url: "https://slow-site.com" }
wait { until_settled: true, ms: 10000 }       [long timeout]
screenshot                                    [check if loaded]
[if still loading]
wait { ms: 5000 }
screenshot                                    [check again]
[if error page]
key { combo: "F5" }                           [reload]
wait { until_settled: true }
```

## Tab management


```
[open a new tab]
key { combo: "ctrl+t" }

[switch to next tab]
key { combo: "ctrl+Tab" }

[switch to previous tab]
key { combo: "ctrl+shift+Tab" }

[close current tab]
key { combo: "ctrl+w" }

[reopen closed tab]
key { combo: "ctrl+shift+t" }
```

## Work with multiple windows


```
[switch focus between tiled windows]
key { combo: "super+Left" }   or   key { combo: "super+Right" }

[close a window]
key { combo: "super+shift+q" }

[toggle fullscreen]
key { combo: "super+f" }

[open terminal alongside browser]
key { combo: "super+Return" }
```

## Extract table data


```
screenshot                                    [see the table]
read_text { region: { x, y, w, h } }         [OCR the table region]
[if text is unclear]
zoom { region: { x, y, w, h } }              [zoom for detail]
[if table is large, scroll and OCR section by section]
```

## Wait for a specific state


```
[wait for a success message]
wait { until_text: "Order confirmed" }
screenshot                                    [verify]

[wait for a page transition]
wait { until_settled: true }
screenshot

[wait a fixed time for animation]
wait { ms: 1500 }
screenshot
```

## Right-click context menu


```
click { x, y, button: "right", describe: "Right-click the image" }
wait { ms: 500 }
screenshot                                    [see context menu]
click { x, y, describe: "Click Save Image As" }
```

## Interact with the desktop (not browser)


```
[open terminal]
key { combo: "super+Return" }
wait { until_settled: true }
screenshot

[type a shell command]
type { text: "ls -la /home/taboom/Downloads" }
key { combo: "Enter" }
wait { ms: 1000 }
screenshot                                    [read command output]

[switch back to browser]
key { combo: "super+b" }
```
