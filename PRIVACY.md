Plainly privacy policy
======================

Plainly keeps everything it records on your PC. It has no account, no
advertising, no analytics and no telemetry, and it never sends what it
records (your timeline, settings, or anything about your apps or files)
anywhere.

Where your data is kept
-----------------------
Everything is stored in %LOCALAPPDATA%\Plainly on this PC. Deleting that
folder, or uninstalling Plainly, removes it.

The only network connections Plainly makes
------------------------------------------
1. Background internet watch (you can turn this off during installation
   or in Settings). Every 30 seconds Plainly opens a connection to
   1.1.1.1 (Cloudflare), 8.8.8.8 (Google) or 9.9.9.9 (Quad9), and if
   those fail, www.msftconnecttest.com (Microsoft's own connectivity
   test), only to see whether the internet is reachable. No data is sent.

2. Update check (you can turn this off during installation or in
   Settings). Once a day Plainly asks api.github.com whether a newer
   version has been released. GitHub sees your IP address and the text
   "Plainly/1.0", like any web request. Plainly never downloads or
   installs updates by itself.

3. Checks you start yourself. When you click "Run the check", "Test speed"
   or "Check it" on the Internet page, Plainly contacts your router,
   1.1.1.1, 8.8.8.8, your DNS server, www.msftconnecttest.com,
   www.cloudflare.com and speed.cloudflare.com, plus any website you type
   in yourself.

Those services have their own privacy policies:
- Cloudflare: https://www.cloudflare.com/privacypolicy/
- Google Public DNS: https://developers.google.com/speed/public-dns/privacy
- Quad9: https://quad9.net/privacy/policy/
- Microsoft: https://privacy.microsoft.com/privacystatement
- GitHub: https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement

What Plainly changes on your PC
-------------------------------
- If you choose "Start Plainly with Windows", it adds one entry to your
  startup list. Uninstalling removes it.
- It only changes other apps' startup settings when you click "Turn it
  off" on a timeline entry, which works the same way as Task Manager's
  Startup tab and can be undone there.
- It only closes a program when you click "End task" and confirm.
