# Privacy

Blink is a screenshot tool, and screenshots often contain the most private things on a screen. So Blink is built to keep them on your PC.

- **No account, no cloud, no analytics or telemetry.** Blink does not collect or send anything about you, your screenshots or how you use the app.
- **Screenshots stay where you put them**: in memory while you edit, on the clipboard when you copy, or in the folder you choose when you save.
- **One network request a day**: Blink asks the GitHub API (`api.github.com`) whether there is a newer release. That request contains no personal data, but like any web request it reaches GitHub with your IP address; [GitHub's privacy statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement) applies to it. You can turn the check off in Settings > Updates. When you install an update, the installer is downloaded from GitHub.
- **Reading text stays on your PC too.** Copying text from the screen and *Find private info* use the OCR engine built into Windows, which works offline. Blink sends no image or text anywhere for it.
- **History** keeps your latest screenshots (20 unless you change it) in `%LOCALAPPDATA%\Blink\history` on your PC, so the tray menu can copy them again. Set it to 0 in Settings > Capture to turn it off and delete them, or use *Clear history* there.
- **Reading QR codes** happens on your PC as well. A link from a QR code only opens in your browser when you `Ctrl` + click it.
- **Settings** are stored in `%APPDATA%\Blink` on your PC.

This website is hosted on GitHub Pages. It uses no cookies, analytics or trackers of its own; GitHub may log visits as described in its privacy statement.

Questions: open an [issue](https://github.com/blink-screenshot/blink/issues).
