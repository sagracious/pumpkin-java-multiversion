# pumpkin-java-multiversion

A multi-version Java Edition protocol translation plugin for [Pumpkin](https://github.com/Pumpkin-MC/Pumpkin) using the Pumpkin WASM plugin API.

## Overview

Pumpkin natively targets the latest Minecraft Java protocol (currently 26.3). This plugin lets older Java Edition clients join the same server. It translates packet ids and payloads in both directions, maps block states, item ids and components, sounds, entity types, particles and tags onto the client's own numbering, and rewrites the packet layouts that changed along the way, including the chunk and light formats used below 1.18.

## Supported Versions

26.3 is the server's own version and needs no translation. Clients from 1.16.2 (protocol 751) up to 26.2 are translated. Anything older is refused at login with a message naming the versions the server supports.

## What gets rewritten

Plenty of packets only need their id remapped. Block updates and section block updates, score updates, the status response and the first three fields of `ADD_ENTITY` carry the same bytes all the way down to 1.16.2, so they pass through untouched.

The rest are rewritten per version. There is no configuration state below 1.20.2, so every registry travels as the NBT dimension codec inside the play login packet, which is what `registry` handles. Tags are four fixed lists up to 1.16.5 and a registry-keyed array from 1.17, in `packet::update_tags`. Chunks carry a varint primary bit mask up to 1.16.5 and a bit set on 1.17, with chunk-wide biomes, full NBT block entities and no section biome palette before 1.18; world height is 0 to 255 up to 1.17.1, so sections outside it are cut rather than shifted. That lives in `packet::chunk_remap` and `packet::chunk_legacy`. Below 1.19 living mobs and paintings arrive in their own spawn packets instead of `ADD_ENTITY`, which is what `packet::legacy` covers.

The 1.21.11 to 1.21.9 step now converts world-border interpolation time from seconds to ticks, drops the new nautilus breathing effect, and tracks game time for wolf/bee anger metadata. Entity stand-ins, new item-component rewrites, and the remaining entity, component, recipe-display, particle, registry, and tag behavior at this boundary are still incomplete.

## Licensing

This plugin is distributed under GPL-3.0-only. Third-party mapping data and attribution are listed in [`assets/NOTICE.md`](assets/NOTICE.md); the GPL text is in [`LICENSE`](LICENSE).
