# Privacy

Blink is a screenshot tool, and screenshots often contain the most private things on a screen. So Blink is built to keep them on your PC.

- **No account, no cloud, no analytics or telemetry.** Blink does not collect or send anything about you, your screenshots or how you use the app.
- **Screenshots stay where you put them**: in memory while you edit, on the clipboard when you copy, or in the folder you choose when you save.
- **One network request a day**: Blink asks the GitHub API (`api.github.com`) whether there is a newer release. That request contains no personal data, but like any web request it reaches GitHub with your IP address; [GitHub's privacy statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement) applies to it. You can turn the check off in Settings > Updates. When you install an update, the installer is downloaded from GitHub.
- **Reading text stays on your PC too.** Copying text from the screen and *Find private info* use the OCR engine built into Windows, which works offline. Blink sends no image or text anywhere for it.
- **History** keeps your latest screenshots in `%LOCALAPPDATA%\Blink\history` on your PC, so the tray menu can copy them again. By default it keeps at most 10, each for at most 24 hours, and then deletes them. In Settings > Capture you can change both, turn History off (0 screenshots, which deletes them) or use *Clear history*.
- **Reading QR codes** happens on your PC as well. A link from a QR code only opens in your browser when you `Ctrl` + click it.
- **Screen recordings** stay on your PC. The video you save goes to the folder you choose (Videos\Blink by default). To let you trim it and switch the pointer and highlights on or off afterwards, Blink also keeps a working copy for a day in `%LOCALAPPDATA%\Blink\recordings`: the video without the pointer and an event log with where the pointer was and where you clicked. After a day, or when you have made more than five recordings, it is deleted.
- **Keys in recordings** are off by default. Only when you switch on *Show keys and clicks in recordings* does Blink watch the keyboard while it records, and only then do keys go into that event log. With *Shortcuts and clicks only* (the recommended choice) it keeps shortcuts such as Ctrl+C and keys such as F5 or Enter, never what you type, so text and passwords stay out. With *All keys and clicks* everything you type is kept and shown in the video, passwords included; Blink warns you when you pick it. Keys are deleted with the working copy and are never sent anywhere.
- **Settings** are stored in `%APPDATA%\Blink` on your PC.

This website is hosted on GitHub Pages. It uses no cookies, analytics or trackers of its own; GitHub may log visits as described in its privacy statement.

Questions: open an [issue](https://github.com/blink-screenshot/blink/issues).
