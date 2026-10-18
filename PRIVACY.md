# Privacy

Flare has no accounts, no analytics, no telemetry and no crash reporting. It keeps your settings, open tabs and a diagnostics log on your own PC (`%APPDATA%\Flare`), and nothing about you or what you do in it is sent anywhere.

## The diagnostics log

To find out why the app froze or failed, Flare keeps a log on your PC (`%APPDATA%\Flare\logs`, at most 10 MB, the oldest part removed as it grows): when the window or the app was slow and for how long, errors and panics, and a timeline of what you did in the app (a tab opened, closed or switched, a Settings page, an update, dictation starting and stopping, a shell starting and ending), and what each tab's agent indicator showed and why (states such as working or finished, never what is on the screen, in a title or in the conversation). Folder paths are written in full. What you type, what you copy, what you say and what a program prints on the screen are never written to it. The log is **never sent anywhere**; it leaves your PC only if you send it yourself. Settings > Diagnostics opens its folder, and has a switch for more detail.

## What reaches the network

Flare makes **no request on its own** in this version. The update check is built but switched off until the project is public; this file changes in the same release that switches it on.

Everything else happens **only when you ask for it**:

- **Installing an update:** not available yet. When it is, clicking Install downloads that release's installer from GitHub.
- **Dictation** (off by default): turning it on and choosing a model downloads the speech engine from whisper.cpp's GitHub releases and the model from Hugging Face; the optional NVIDIA acceleration pack is a separate download you choose. Each file is checked against a fixed SHA-256 before use. Your voice never leaves the PC: it goes to a speech engine running on your own machine (127.0.0.1, or straight into the engine's input for the Parakeet model) and is never written to disk.
- **Links:** clicking a link printed in the terminal opens it in your browser. Nothing is opened or fetched before you click.
- **Sonara** (the text-to-speech reader for Claude Code, if you run it): when Claude Code's question widget in a tab changes question, Flare tells the Sonara running on your own PC (127.0.0.1, at the address and with the key in Sonara's own `runtime.json`) the question showing and a random id of that tab, so Sonara can read the question you moved to. Without Sonara nothing is sent. Each tab's shell carries that id as `FLARE_TAB_ID`.
- **What you run:** the terminal runs your own shell, and whatever you run in it reaches the network as you tell it to.

## What it adds to Claude Code

Flare adds a small local plugin, `flare-status`, to the Claude Code sessions started in its tabs, so the tab can show what the agent is doing: working, waiting on you, finished or failed. The plugin's hooks print one fixed line (`OSC 777;flare-agent`) into that tab's own terminal; they send nothing over the network, read nothing, and write no file. It is passed through an environment variable of the tab's shell, never written into your Claude Code settings. Settings > Appearance > "Exact status from Claude Code" turns it off for every terminal opened afterwards.

## What programs in a tab can do

A program running in a tab (Claude Code's /copy, or anything you reach over ssh) can put text ON your clipboard with the standard OSC 52 sequence, up to 1 MB, and a "Copied" badge shows when it does. Nothing in a tab can READ your clipboard that way: such a request gets no answer. The text a program copies is never written to the diagnostics log. When a program asks, the terminal also tells it its own name and version (XTVERSION, `Flare <version>`) and the colours it is painted in, so it can draw to match; that answer goes only to the program that asked, on your PC.

If the app ever gains a request that is not listed here, this file changes in the same pull request.
