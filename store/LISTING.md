# Orbit: Microsoft Store listing

Paste these into Partner Center. Screenshots are in `store/screenshots` (1920 x 1080, in order).

**Category:** Productivity (or Utilities & tools) · **Price:** Free · **Markets:** all

**Privacy policy URL:** https://github.com/GalacticVraj/orbit/blob/main/PRIVACY.md
**Website:** https://github.com/GalacticVraj/orbit
**Support contact:** https://github.com/GalacticVraj/orbit/issues

## Short description

The fastest way to act on files. Select files in File Explorer, press Alt + Q, and a glass ring of actions blooms around your cursor: compress, convert, resize, merge, rename. Everything runs on your PC.

## Description

Stop uploading your files to random converter websites. Orbit puts the actions you need one key away, right where your files are.

Select one or more files in File Explorer and press Alt + Q. A liquid-glass ring opens around your cursor, showing only the actions that fit what you selected. Pick one and it's done, saved next to your original.

• PDF: compress, split pages, extract images, merge PDFs, convert to Word (with Microsoft Office)
• Images: convert to JPG, PNG or WEBP, resize, compress
• Videos: compress, convert to MP4, extract the audio
• Word and PowerPoint: convert to PDF, export slides as images (requires Microsoft Office)
• Folders and files: zip, calculate size
• Smart Rename: rename 100 files in one go with a live preview, and undo it with one click

Your originals are never overwritten. Results get a new name like "report (2).pdf", and Orbit selects them for you in File Explorer.

Private by design: no uploads, no account, no ads, no internet connection. Light by design: 0% CPU while it waits for Alt + Q, and the ring opens almost instantly.

## What's new in this version

First release on the Microsoft Store.

## Product features (one per line, max 20)

Alt + Q opens a ring of actions for the selected files
Compress, split and merge PDFs
Convert PDF to Word (with Microsoft Office)
Convert and resize images (JPG, PNG, WEBP)
Compress and convert videos, extract audio
Word and PowerPoint to PDF
Batch rename with live preview and undo
Zip folders, calculate sizes
Works on one file or hundreds
Never overwrites your originals
100% offline, no uploads, no account
Light and dark themes

## Search terms (max 7)

pdf compressor · image converter · file converter · batch rename · compress video · resize images · merge pdf

## Copyright

© 2026 GalacticVraj

## Age rating questionnaire (IARC)

Answer **No** to everything: no violence, no user-to-user communication, no sharing of location or personal info, no purchases, no gambling, no ads.

## Restricted capability justification (runFullTrust)

Orbit is a classic Win32 desktop app (Rust, Win32, Direct2D) packaged with the Desktop Bridge. It needs full trust to register its global hotkey (Alt + Q), read the current File Explorer selection through the Shell COM interfaces, draw its ring as a layered window, and convert files with Media Foundation and Office automation.

## Submission notes for certification

Orbit starts in the system tray. Select one or more files in File Explorer (for example a PDF or a few images) and press Alt + Q. Click an action in the ring. Esc closes the ring.
