# Milestone 08: Postprocessing

## Goal

Add FFmpeg-backed postprocessing parity for common media workflows.

## Deliverables

- [x] Merge audio/video
- [x] Remux
- [x] Extract audio
- [x] Embed metadata
- [x] Embed thumbnails
- [x] Subtitle conversion

## Implemented Scope

- [x] Multi-source download planning for merge selectors such as `bestvideo+bestaudio`.
- [x] FFmpeg-backed merge execution for separate video and audio downloads.
- [x] `download --remux-video <ext>` for container remuxing.
- [x] `download --extract-audio --audio-format <fmt>` for common audio outputs.
- [x] `download --embed-metadata` for title, channel, and description metadata injection.
- [x] `download --embed-thumbnail` for cover art embedding on supported outputs.
- [x] `download --subtitles --convert-subs <fmt>` for subtitle artifact conversion.
- [x] Final media and generated subtitle artifacts persist through the existing artifact/archive ledger path.

## Acceptance Criteria

- [x] Postprocess plans execute deterministically.
- [x] Output files and artifact records stay in sync.
