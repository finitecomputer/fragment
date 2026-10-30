---
title: Shipping the picker firmware
summary: How a firmware release goes out to the orchard fleet
tags: [deploy, firmware, fleet]
---
# Shipping the picker firmware

## Release train

Firmware ships every second Tuesday. The release captain tags the build, signs it with the fleet key and stages it to ten percent of pickers first.

## Reverting a bad release

If the canary pickers report bruised fruit or stalled arms, the captain reverts: re-point the fleet channel at the previous signed build and announce it in #fleet. Reverting takes about four minutes for the whole fleet.

## Who can ship

Only the release captain for that week, listed on the rota page.
