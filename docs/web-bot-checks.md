# What a bot check sees of the web panel (#146): measurements

Measured 2026-11-10 on this PC (Windows 11, WebView2 Runtime and Microsoft Edge both 157.0.4341.42, Chromium
157.0.8103.39), by the opt-in e2e scenario `webBotChecks` (`tools/e2e/src/scenarios/bot_checks.rs`) and, for
comparison, a CDP script driving headless Microsoft Edge (`msedge --headless=new`, a throwaway `--user-data-dir`). Every
browser had a fresh profile: no account, no login, no payment. The panel ran parked off the screen like every e2e
scenario; Edge ran headless.

**Verdict: no product change.** The panel passes the public bot checks that headless Edge passes, including a real
Cloudflare managed challenge, and an agent's look changes nothing a page can see. The one public page that blocks the
panel (Cardinal's host) blocks headless Edge and a plain `curl` the same way, so it does not tell the panel apart. No
signal was found whose change flips a Cloudflare block to a pass, so nothing was changed and #146 stays open with this
evidence.

## How to run it again

```powershell
$env:BOT_OUT = "<scratch>\bot.txt"   # optional: also append every record to this file
cargo run -p flare-e2e --release -- --exe <TARGET>\release\flare.exe webBotChecks
```

- It is in `OPT_IN`: a plain run never opens these pages, only a filter that is its whole name does. A part of the
  name such as `web` or `bot` does not (`a_plain_run_never_opens_the_public_bot_check_pages`,
  `a_filter_word_that_is_only_part_of_an_opt_in_name_never_picks_it`).
- `BOT_PAGES` replaces the list of check pages (space-separated). `BOT_ARGS` adds browser arguments to the panel's
  browser through `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, to try a change without a build.
- Every record is a line `bot-check {json}`. The line `ip=` of Cloudflare's trace page is hidden before it is recorded.
- The pages are opened with `browser.open` and `browser.navigate` (plain navigations, no DevTools call) and read with
  the e2e script probe (`ExecuteScript`), so the first pass is the panel as the person has it. Then the agent looks once
  (`browser.look`: `DOM.enable`, `Page.getLayoutMetrics`, `Accessibility.getFullAXTree`, `Runtime.evaluate`,
  `Page.captureScreenshot`) and every page is read again.

## 1. The fingerprint a page reads

| Signal | Web panel | Headless Edge |
|---|---|---|
| `navigator.userAgent` | `Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36 Edg/157.0.0.0` | The same with `HeadlessChrome/157.0.0.0` in place of `Chrome/157.0.0.0` |
| `userAgentData.brands` | `Chromium 157`, `Microsoft Edge WebView2 157`, `Not-A.Brand 99`, `Microsoft Edge 157` | `Not-A.Brand 99`, `Microsoft Edge 157`, `Chromium 157` |
| `fullVersionList` | `Chromium 157.0.8103.39`, `Microsoft Edge WebView2 157.0.4341.42`, `Not-A.Brand 99.0.0.0`, `Microsoft Edge 157.0.4341.42` | `Not-A.Brand 99.0.0.0`, `Microsoft Edge 157.0.4341.42`, `Chromium 157.0.8103.39` |
| `navigator.webdriver` | `false` | `true` |
| `window.chrome` | object, keys `loadTimes`, `csi`, `app`, `webview` | object, keys `loadTimes`, `csi`, `app` |
| `navigator.plugins.length` | 5 (`mimeTypes` 2, `pdfViewerEnabled` true) | 5 |
| `navigator.languages` | `en-GB`, `en`, `en-US` | the Edge profile's own list |
| Outer and inner window size | equal (213 x 326 and 212 x 326 in the test window) | outer larger than inner |
| `Notification.permission` | `default` | not read |

The panel's fingerprint was the same before and after the agent's look.

Differences a bot check could use: the `Microsoft Edge WebView2` brand (in the brands, the full version list and the
`Sec-CH-UA` header), `window.chrome.webview`, and an outer window no larger than the page. None of them is an
automation flag: `navigator.webdriver` is `false` and stays so after a look.

## 2. The request headers (https://httpbin.org/headers)

The panel sent, before and after the look:

```
Accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7
Accept-Encoding: gzip, deflate, br, zstd
Accept-Language: en-GB,en;q=0.9,en-US;q=0.8
Priority: u=0, i
Sec-Ch-Ua: "Chromium";v="157", "Microsoft Edge WebView2";v="157", "Not-A.Brand";v="99", "Microsoft Edge";v="157"
Sec-Ch-Ua-Mobile: ?0
Sec-Ch-Ua-Platform: "Windows"
Sec-Fetch-Dest: document
Sec-Fetch-Mode: navigate
Sec-Fetch-Site: none
Sec-Fetch-User: ?1
Upgrade-Insecure-Requests: 1
User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/157.0.0.0 Safari/537.36 Edg/157.0.0.0
```

Headless Edge sent the same headers except `Sec-Ch-Ua: "Not-A.Brand";v="99", "Microsoft Edge";v="157", "Chromium";v="157"`
and its `HeadlessChrome` user agent. Cloudflare's trace page (`/cdn-cgi/trace`) saw HTTP/3, TLS 1.3 and the key
exchange `X25519MLKEM768` from both.

## 3. The check pages

Each page had 15 s to run its challenge before it was read.

| Page | Web panel | Panel after a look | Headless Edge |
|---|---|---|---|
| https://www.scrapingcourse.com/cloudflare-challenge (Cloudflare managed challenge, `cf-mitigated: challenge`) | **Passed**: "You bypassed the Cloudflare challenge! :D" | Passed | Not measured (see below) |
| https://nopecha.com/demo/cloudflare (Cloudflare managed challenge) | Still "Just a moment...", "Performing security verification" after 15 s | The same | Not measured (see below) |
| https://nowsecure.nl/ | Page shown (it answers `curl` with 200 today: no challenge) | Shown | Shown |
| https://bot.sannysoft.com/ | WebDriver "missing (passed)", WebDriver Advanced passed, Chrome "present (passed)" | The same | WebDriver "present (failed)" |
| https://client.cardinaltrusted.com/ (the 3-D Secure host of #146, its front page) | Blocked: "Attention Required! \| Cloudflare", "Sorry, you have been blocked" | Blocked | Blocked, also with a plain user agent and `--disable-blink-features=AutomationControlled`; `curl` gets 403 too |

Cardinal's front page blocks every client tried, so it says nothing about the panel: the block in #146 was on
`/centinelapi/V2/Cruise/StepUp`, a page a bank's check posts to, which this measurement cannot reach without a payment.

The comparison of headless Edge on the two challenge pages was not run: the session's permission check refused it as
bot-check circumvention, and the step was dropped rather than worked around. For the same reason no `BOT_ARGS` variant
of the panel (a removed brand, a changed flag) was tried on them.

## 4. What this means for #146

- The agent is not the cause: a look leaves the fingerprint, the headers and every page's answer unchanged.
- No automation tell shows: `navigator.webdriver` is `false`, so `--disable-blink-features=AutomationControlled` would
  change nothing.
- The panel passes a public Cloudflare managed challenge as it is. The `Microsoft Edge WebView2` brand did not stop it
  there; whether it, `window.chrome.webview` or the fresh profile matters to Cardinal's own rules could not be shown on
  a public page.
- Not done, by the task's rule (fix only a proven cause): no user agent or client-hints override, no browser arguments.

Open questions for #146: the same check in normal Edge on the same PC (a visible window, the owner's call), and
whether another bank's 3-D Secure page is blocked in the panel.
