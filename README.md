This is the source code for <https://midos.house/>, a platform for organizing [Ocarina of Time randomizer](https://ootrandomizer.com/) events like tournaments and community races. It integrates with other platforms to automate some aspects of event management, including:

* [racetime.gg](https://racetime.gg/) (creating official and practice race rooms, handling chat commands, handling results and [FPA](https://docs.google.com/document/d/e/2PACX-1vQd3S28r8SOBy-4C5Lxeu6nFAYpWgQqN9lCEKhLGTT3zcaXDSKj0iUnZv6UPo_GargUVQx5F-wOPUtJ/pub) calls)
* [Discord](https://discord.com/) (creating scheduling threads, handling commands for scheduling and settings drafting, posting results, notifying organizers when their attention is needed)
* [start.gg](https://start.gg/) (handling matchups, reporting results)
* [ootrandomizer.com](https://ootrandomizer.com/) (generating seeds, unlocking spoiler logs after races)
* [Mido's House Multiworld](https://github.com/midoshouse/ootr-multiworld) (creating official rooms, auto-tracking integration for restreams)
* and many more.

Mido's House is custom-built in close cooperation with event organizers to allow it to work with the features that make each event unique. For example, it supports the [Triforce Blitz](https://www.triforceblitz.com/) format's time limit, scoring system, and seed generator.

# Contributing

If you're interested in contributing to the Mido's House project, feel free to contact me on Discord (@fenhl). Here are some ways you could help out:

## Code

The Mido's House codebase is currently a one-person project, but it doesn't have to remain that way! If you're interested in contributing to the codebase but don't know where to start, let me know so we can discuss how you can help.

## Data archival

I'm also always looking for trusted members of the OoTR community willing to help out as “archivists”, i.e. for the task of manually adding race room and restream/vod links to races for events where this isn't automated. We have an invitational Discord channel for coordinating this.

## Translations

Mido's House currently has an incomplete French translation which was created for the [Tournoi Francophone Saison 3](https://midos.house/event/fr/3) by that event's organizers. Let me know if you would like to proofread or extend this translation, or start a new one. A priority of the project is attention to detail, so I would include custom code to handle grammatical variations like case and gender where necessary. For example, the English Discord message for race results uses the word “defeat” or “defeats” depending on whether the winning team's name is grammatically singular or plural.

# Dev notes

Discord invite link with appropriate permissions (only useable by members of the Mido's House Discord developer team):

* Dev: <https://discord.com/api/oauth2/authorize?client_id=922793058326691901&scope=bot&permissions=318096427008>
* Production: <https://discord.com/api/oauth2/authorize?client_id=922789943288410152&scope=bot&permissions=318096427008>
