# pumpkin-java-multiversion

A multi-version Java Edition protocol translation plugin for [Pumpkin](https://github.com/Pumpkin-MC/Pumpkin) using the Pumpkin WASM plugin API.

## Overview

Pumpkin natively targets the latest Minecraft Java protocol (currently 26.3). This plugin lets older Java Edition clients join the same server. It translates packet ids and payloads in both directions, maps block states, item ids and components, sounds, entity types, particles and tags onto the client's own numbering, and rewrites the packet layouts that changed along the way, including the chunk and light formats used below 1.18.

## Advertised Client Range

26.3 is the server's own version and needs no translation. The login gate currently accepts clients from 1.16.2 (protocol 751) through 26.2 and refuses older clients. The end-to-end ViaVersion/ViaBackwards parity audit for every packet family and version edge is still in progress; an accepted version is not yet proof that all gameplay packets are translated correctly.

## What gets rewritten

Plenty of packets only need their id remapped. Block updates and section block updates, score updates, the status response and the first three fields of `ADD_ENTITY` carry the same bytes all the way down to 1.16.2, so they pass through untouched.

The rest are rewritten per version. There is no configuration state below 1.20.2, so every registry travels as the NBT dimension codec inside the play login packet, which is what `registry` handles. Tags are four fixed lists up to 1.16.5 and a registry-keyed array from 1.17, in `packet::update_tags`. Chunks carry a varint primary bit mask up to 1.16.5 and a bit set on 1.17, with chunk-wide biomes, full NBT block entities and no section biome palette before 1.18; world height is 0 to 255 up to 1.17.1, so sections outside it are cut rather than shifted. That lives in `packet::chunk_remap` and `packet::chunk_legacy`. The 1.18 bundled light data is split into a `LIGHT_UPDATE` before the legacy chunk; the 1.16 light masks are cropped to that client's 18 sections and encoded in its VarInt layout. Below 1.19 living mobs and paintings arrive in their own spawn packets instead of `ADD_ENTITY`, which is what `packet::legacy` covers.

The 1.21.11 to 1.21.9 step converts world-border interpolation time from seconds to ticks, drops the new nautilus breathing effect, tracks game time for wolf/bee anger metadata, and maps Nautilus, Zombie Nautilus, Camel Husk, and Parched spawns and tags to older entity types. Nautilus-only metadata is filtered, Camel Husk fields map onto Camel metadata, and entity hover names use the same stand-ins. Recipe displays translate all 11 slot-display codecs emitted by Pumpkin's 26.3 writer for clients from 1.21.2 onward; slot/item IDs and holder sets are rewritten. The legacy recipe-book bridge below 1.21.2 tracks unlocked recipes and settings per connection and retains ViaBackwards' known lossiness: locked recipes are unavailable, smithing recipes are omitted, and some slot displays are approximated.

Chunk palettes now have a legacy repack path for global/direct palettes that older 1.17.1 and 1.16.2 readers cannot accept as-is. Pumpkin writes standalone light updates in each connected client's layout; PJM splits the bundled 1.18 light block into a separate packet before sending the legacy chunk. The earlier console lines about relighting copied chunks are server log activity, not a PJM light-packet decoder error. `STOP_SOUND` keeps names unchanged when the target's numeric sound map contains the vanilla sound, preserves custom names, and cancels a vanilla selector when that sound has no target entry. Via's separate identifier-level rename table is not in PJM's vendored mappings, so renamed/aliased sound names are not yet fully equivalent to Via.

Item particles rewrite their nested stack; 1.17/1.18 level-particle vibrations rebuild the origin block position from packet coordinates. For clients before 1.20.5, entity-effect color moves into the particle speed field, and legacy area-effect-cloud colors discard alpha. Trim, instrument, and provides-trim-material components use versioned holder IDs, including the 1.21.5 `EitherHolder` forms; legacy item NBT carries instrument, trim material, and trim pattern names. Custom model data uses its four-array shape from 1.21.4 and downgrades to the legacy integer only when that conversion is exact. Their 26.3 codecs and item hashes are being corrected in the matched Pumpkin fork. Static vanilla registries are covered; custom inline registry values cannot yet be represented and fail closed. The item-backup path covers lossy structured and legacy-NBT downgrades, including item stacks nested in entity metadata and particles. As in ViaBackwards, clients below 1.21.5 send full stacks that are reduced to bare item IDs and counts before Pumpkin reads container clicks, so changed component details cause a server resync rather than a fully translated click. Other legacy item component conversions, runtime/dynamic chat-type registries, and broader entity/advancement/tag semantics remain under audit.

Everything in the current audit batch is still unbuilt. The accepted login range is a version gate, not a verified feature-parity claim, until consolidated GitHub Actions pass and the direct version-boundary joins are checked.

## Licensing

This plugin is distributed under GPL-3.0-only. Third-party mapping data and attribution are listed in [`assets/NOTICE.md`](assets/NOTICE.md); the GPL text is in [`LICENSE`](LICENSE).
