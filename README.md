# Localify

**A local music player for Windows, with Spotify's feel and a library that's actually yours.**

![Rust](https://img.shields.io/badge/Rust-stable-000?logo=rust)
![Tauri](https://img.shields.io/badge/Tauri-v2-24C8DB?logo=tauri)
![Windows](https://img.shields.io/badge/Windows-10%2F11-0078D4?logo=windows)
![License](https://img.shields.io/badge/license-GPL--3.0-blue)

---

## What it is

A local music player that gets its music from free, anonymous sources.

Music catalogues — YouTube Music, MusicBrainz — tell it what songs exist and who
made them. yt-dlp gets the audio from YouTube. Neither one needs an account, a
subscription, or anything about you.

Underneath, it's a front end for yt-dlp: it decides *which* video is the song
you asked for, and yt-dlp does the fetching. What makes it a *player* rather
than a downloader is that **you never see any of that**. There's no download
button, no download manager, no queue to babysit. You press play and it plays —
the first time too. Your library builds itself, into ordinary files on your own
disk, while you listen.

It's a personal player: one person, one machine, nothing shared with anyone.
[More on that below](#what-this-is-precisely).

```
  Search  ──▶  Press play  ──▶  It plays
                    │
                    └── (underneath: match, download, verify, tag)
```

---

## Getting around

**The sidebar** holds Home, Search, Songs and Liked Songs, then your playlists.
Drag its edge to resize it, or collapse it to icons.

**Home** leads with things you haven't heard: songs picked from your artists and
genres, *because you listened to* suggestions, and favourites you haven't played
in months. Your history — what's on repeat, what you played last, your top
albums and artists — comes after. Sections with nothing worth showing don't
appear at all, so Home grows as your library does.

**Search** returns songs, albums and artists, with a top result picked by
popularity. Versions of the same song fold into one row so twenty karaoke
uploads don't bury the original. There's also a quick bar for when you know
exactly what you want: type, hit enter, it plays.

**Songs**, **Liked Songs**, albums, artists and playlists all share the same
list: number, cover, title with the artist underneath, album, date added and
length. Right-click any row for play next, add to queue, add to a playlist,
like, go to album or artist, or delete its download.

**The player bar** sits at the bottom: shuffle, previous, play/pause, next,
repeat, and on the right the queue, full-screen now playing, and volume. Click
the cover to expand it, with synced lyrics when they exist.

**The queue panel** shows what's playing, what's next, and where the rest is
coming from. Drag to reorder it.

---

## What it does

**Playback** · Spotify's two-tier queue — the queue you build always wins over
the album you're playing · shuffle with a stable permutation, so going back
doesn't reshuffle · repeat track and repeat queue · configurable crossfade, and
real gapless when it's off · 10-band equalizer with profiles · remembers the
exact second you left off, across restarts.

**Library** · Instant search across tens of thousands of tracks · lists that
stay at 60 fps no matter how long they get · favourites, albums and artists ·
reconciliation between what's on disk and what's in the database.

**Playlists** · Create, rename, describe, drag to reorder · import public
playlists from **Spotify** and **YouTube Music**, with their name, cover and
description · per-playlist suggestions.

**Recommendations** · The judgement happens on your machine: your artists, your
genres, your playlists, what you haven't played in months. It asks the catalogue
*"what else does this artist have?"* — the same question the search box asks —
but nothing about your listening ever leaves the machine, and there's no
recommendation service to send it to.

**Integrations** · Windows media panel with cover art · media keys · Discord
Rich Presence showing what's playing · Last.fm scrobbling with a **persistent
queue**, so going offline doesn't cost you a single scrobble.

**Also** · Synced lyrics where they exist · English and Spanish · dark theme.

---

## Settings

**Music catalogue** — where search results and track data come from. YouTube
Music knows what's been uploaded; MusicBrainz knows what's been released.
Combined is the default and works best. Audio always comes from YouTube either
way.

**Library folder** — where your music lives. Change it and Localify offers to
move what you already have.

**Audio** — crossfade, equalizer, output device.

**Discord Rich Presence** *(optional)* — needs your own Application ID, a
one-minute job in Discord's developer portal. One isn't bundled on purpose: it
would belong to whoever compiled the binary, and everybody would show up under
the same stranger's application.

**Last.fm** *(optional)* — your own API account, for the same reason.

**Storage** — rescan the library, or delete everything downloaded and start
clean. Your playlists and liked songs survive that; each song downloads again
the next time you play it.

Credentials go into the Windows secret store (DPAPI). They never touch the
database, the bridge to the interface, or the logs.

---

## Running it

Rust is the only thing you need. yt-dlp and FFmpeg download themselves on first
run.

```powershell
cargo build --release -p localify-app
cargo run --release -p localify-app
```

---

## What this is, precisely

**Localify is a front end and library manager for [yt-dlp](https://github.com/yt-dlp/yt-dlp).**
It doesn't fetch anything itself: it works out which video corresponds to the
song you asked for, hands that to yt-dlp, and then organises, tags and plays the
result. Take yt-dlp away and there is no audio.

Everything it does, you could do by hand with yt-dlp and a text editor. What it
adds is that you don't have to.

Concretely, the project:

- **hosts and distributes nothing.** No content ships with it, no server of ours
  serves anything, and nothing you download is shared with anyone.
- **is not a bridge between users.** There is no upload, no seeding, no
  library sharing. Your files stay on your disk.
- **breaks no protection.** It requests the same thing a browser requests. There
  is no DRM stripping and no paywall in the picture — YouTube is free and needs
  no account.
- **is for one person, on their own machine.** It's a personal player, not a
  service.

### Legal notice

That said, whether *your particular use* is lawful depends on the content and on
where you live, exactly as it does with yt-dlp on its own. Downloading a video
may also go against YouTube's Terms of Service, which is a matter between you
and them. Use it for personal listening; the responsibility is yours.

## Contributing

How it's built, why it's built that way, and how to work on it:
**[CONTRIBUTING.md](CONTRIBUTING.md)**.

## License

[GPL-3.0](LICENSE).
