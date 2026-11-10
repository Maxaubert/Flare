# Ask agent additions: page, arrows, file selection, markdown views

Status: proposed for approval, 2026-11-10. Plan: `docs/superpowers/plans/2026-11-10-ask-agent-additions.md`.
Owner's request: ask about the current page; arrows in the browser, drawn by the agent or by the person; select text
in files and ask; `.md` files viewable as compiled, markdown or code, "compiled by default".

## Shared rules

- Only a person's gesture types at the prompt, and it never presses Enter (rule 188, rule 234).
- Page text and file text are untrusted. They go through `ask_agent::gated` and `frame_selection`.
- No ctl, MCP or plugin method can read or send a selection.
- Tests run headless.

## 1. Ask about the current page

Today the button with no selection shows a note and types nothing. The agent can already see that page through
`browser_look`, because the panel is the same WebView2.

**Change.** On a plain web page, the button with no selection sends a reference to the page:

```
About the page open in this tab (example.com):
> title: Pricing
> address: https://example.com/pricing
> (use browser_look to read it)
```

The person types the question after it and presses Enter. With a selection, nothing changes.

- The address and title come from the document that answered, in one script answer. The title is untrusted and gated.
- Mini-app and plugin pages keep the note. Their address means nothing to the agent.
- The button's tooltip says "Ask agent about this page" when nothing is selected, if a selection can be known cheaply.
  Otherwise it says "Ask agent about the selection or this page".

## 2. Arrows

The arrow is a native window above the page, not part of the page. WebView2 is a native child window, so Slint can't
draw over it. Flare already draws the agent cursor this way (`agent_cursor.rs`). An in-page arrow could be read, moved
or blocked by the page.

### Agent to person

New tool `browser_point {ref | x,y, label?, clear?}` in the Browser namespace, with the same grant as the other browser
tools. It draws one orange arrow per tab, with an optional short label (cleaned, 48 characters). It does not click and
cannot type.

- It scrolls the target into view first and answers at once with `{pointed, offscreen}`.
- It follows scroll and resize by measuring about every 100 ms. It can trail a fast scroll slightly.
- It goes away on: `clear`, a new arrow, the person clicking or typing in the page, navigation, closing the panel or
  tab, or after 60 s. It hides under Flare menus and behind other tabs, as the cursor does.
- The tool text tells agents when to use it: "the person must do this step themselves".

### Person to agent

A "Draw arrow" button beside Ask agent, in Flare chrome. It shows only on a tab with an agent, on web and mini-app pages,
not plugin pages.

1. The person clicks it. A transparent pad covers the page and the pointer becomes a crosshair.
2. The person drags from tail to head. A blue arrow follows live.
3. On release, Flare asks the browser, without running page script, what element is under the head.
4. Flare pastes a frame at the prompt, unsent, and focuses the terminal:

```
About this arrow on example.com:
> points at: button "Copy key"
> at: 412, 300 in a 1024 x 768 page
> (use browser_look to see it)
```

The person adds "explain this" and presses Enter.

Flare keeps the arrow for 5 minutes or until navigation. The next `browser_look` gets the arrow's details and a
screenshot with the arrow drawn on it, so the agent sees what the person sees.

Cancel: the button again, right click, a menu, a tab switch or navigation. Esc too, where it arrives.

**Check first.** Whether a layered window can catch the mouse without taking focus from the page is not proven. A
headless probe comes first. If it fails, the person's arrow becomes a single click on the target instead of a drag, and
the rest stays the same.

## 3. Select text in files

The Text panel gets real selection: drag, shift-click, double-click for a word, triple-click for a line, Shift with arrows,
Ctrl+A, Ctrl+C. Rust holds the selection as line and character positions, so it survives scrolling, wrapping and splitter
drags. Slint only reports the pointer and paints each row. A read-only `TextInput` can't work across recycled rows.

- Ask agent with a selection sends the quote, labelled like `notes.md lines 12-40`. With no selection it still sends the path.
- Esc clears a selection first and closes the panel only when nothing is selected.
- A drag selects. The wheel, keys and scrollbar scroll.
- A selection joins soft-wrapped rows of one line. Only real line ends become line breaks.
- The text is what the panel shows: tabs as four spaces and very long lines cut. The docs say so.

The Changes panel is separate code and is not part of this.

## 4. Markdown views

A `.md` or `.markdown` file opens **compiled** by default, every time. A control in the panel header switches between:

| View | What you see | How |
|---|---|---|
| Compiled (default) | Formatted document | Flare-written page |
| Markdown | The text with markdown coloured | Flare-written page |
| Code | Today's text with line numbers and wrap | The native Text panel, with selection from item 3 |

A panel keeps its own view across a restart. Files the agent opens with `show_file` follow the same default.

**Security.** The file is untrusted. The page has no script: `default-src 'none'; style-src 'unsafe-inline'; img-src data:`.
Flare writes every tag itself, and raw HTML in the file shows as text. If the WebView2 runtime is missing or rendering
fails, the panel falls back to Code.

**Not in this version.** Links that leave the page (they show their address as a tooltip; `#` jumps inside the page work).
Pictures (they show their alt text). Both need their own safety rules: opening an address from an untrusted file, and
reading local image files.

Ask agent works on selected text in the two page views, with the file name as the label.

## Not in this set

Arrows on plugin pages, an arrow in the terminal grid, several arrows at once, freehand drawing, colouring for code files
other than markdown, Changes panel selection, a `view` argument for agent tools.
