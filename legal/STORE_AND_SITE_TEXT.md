# Short legal text for the Steam page and the launch page

Paste-ready. Replace the [BRACKETS] first, the same way as in the two documents.

## Steam page

Steam has three places for this.

**1. Legal line** (Steamworks, Store page admin, Basic info, "Legal line"):

> © 2026 [OPERATOR NAME]. Legion of Minds. 3D characters and buildings by Kay Lousberg (KayKit, CC0).

**2. EULA / Terms** (Steamworks, Store page admin, "Custom EULA"). Steam shows it before first launch, so either paste the full Terms of Service there, or leave Steam's default and rely on the in-game "Before you play" screen. Pasting it is clearer for players.

**3. "About this game" closing paragraph** (also the place to say what data is collected, which Steam's review expects):

> **Online play and your data.** Offline play sends nothing anywhere: your troops and saves stay on your PC. Online play (raids, the weekly cup, clans) needs a free Google or Discord sign-in and is for players aged 13 and up. When you play online, your published base and the armies you send, including their trained brains, are stored on our server so battles can be checked and other players can raid you. Sharing anonymous play data to help improve the game is optional and off unless you turn it on. You can delete your online account in Settings at any time. Terms of Service: [SITE]/terms.html · Privacy Policy: [SITE]/privacy.html

**Content survey answers that follow from the policy**: the game has user-generated content shown to others (player and clan names, clan chat); it collects no email and has no in-game purchases (yet).

## Launch page (site/index.html)

The footer already links to `terms.html` and `privacy.html` (built from `legal/` by `python3 legal/build_site_pages.py`). For the download section, one line under the button:

> Free sign-in with Google or Discord for online play, ages 13+. Offline play needs no account. By downloading you agree to the [Terms of Service](terms.html); see how we handle data in the [Privacy Policy](privacy.html).

## Discord server (rules channel, optional)

> Legion of Minds' [Terms of Service]([SITE]/terms.html) apply here too: no cheats or exploits, no hate or harassment, no selling accounts. Report bugs instead of using them.
