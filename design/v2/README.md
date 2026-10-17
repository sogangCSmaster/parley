# Design v2: "If Apple made Tolkmeet"

The target look for the next version, explored on a design canvas (2026-10-17). These are **mockups**, not shipped UI. The current app still follows [../DESIGN_SYSTEM.md](../DESIGN_SYSTEM.md) until each screen is rebuilt.

- `screens/`: a PNG of every screen (rendered from `source/`). Page-type screens (`MainFull`, `Privacy`, `Terms`, `Refund`) are full-page captures.
- `source/`: the canvas sources (`*.dc.html` + `canvas.json`), kept so the mockups can be edited again. They need the canvas runtime (`support.js`), so they do not open on their own. Read them for exact sizes, colors and copy.

## What changes from v1

| Topic | v1 (current app) | v2 |
|---|---|---|
| Type | Pretendard JP | System font (`-apple-system`, SF Pro / Apple SD Gothic Neo), Pretendard as fallback on other OSes |
| Surfaces | Thin borders, flat panels | No borders; layers come from fill (`#F5F5F7` page, `#FFFFFF` cards), large radii (14–30px), translucent glass (`backdrop-filter`) over video |
| Filled buttons | Ink | **Deep orange `#D4401C` with white text and white icons** (4.62:1), also for icon tiles and badges on orange. The brand orange `#F2542D` stays for non-text accents only: logo, translation pill, speaking ring, progress |
| Camera off | Colored initial circle | Gray monogram (`#A2A6B0 → #7D818C`), speaking = orange double ring + level bars |
| Captions | Panel list, same size for every line | Recency sizing: the latest line is largest and darkest, older lines shrink and fade. Scrolling up switches to uniform reading mode with a "최신 대화로" pill |
| Meeting room | Caption panel on the right | Video tiles + a light "대화 기록" column (a mini version of the live captions screen); one floating control capsule |
| Screen share (presenter) | Captions hidden behind the shared app | Document Picture-in-Picture caption window, always on top, never shared |

## Screens

| Group | Screens |
|---|---|
| Meeting | `Main` (hero), `Meeting`, `MeetingShare` (presenter, fullscreen + PiP), `Lobby` (guest waiting room), `Knock` (host admit prompt), `Ended` |
| Captions | `Captions` (live), `CaptionsReview` (scrolled up), `InPerson` (one phone, two people) |
| Errors | `Err404`, `ErrExpired` (meeting link closed), `ErrMic` (mic denied), `ErrBrowser` (in-app browser) |
| Account | `SignUp`, `Verify` (6-digit code + spam folder hint), `SignIn`, `Mfa` (authenticator code), `Passkey`, `ResetRequest`, `ResetNew` |
| Billing | `Checkout`, `PaidDone`, `Quota` (monthly hours used up) |
| Signed in | `Home`, `Meetings`, `Schedule`, `ScheduleDone` (copy invite: schedule + link to clipboard), `Account`, `Security` (passkeys, 2FA, devices) |
| Pages | `MainFull` (landing), `Privacy`, `Terms`, `Refund` (English, drafts for legal review) |
| Email | `MailCode`, `MailReset`, `MailInvite` (written in the guest's language), `MailReceipt` |

Placeholders in brackets (`[회사명]`, `[DATE]`, `[수치]`, `[QR 코드]`) are facts not decided yet. Names and email addresses in the mockups are fictional.
