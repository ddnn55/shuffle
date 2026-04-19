#!/bin/zsh
cd '/Users/davidstolarsky/Development/mp3player'
export SHUFFLE_MEDIA_EVENTS_LOG='/Users/davidstolarsky/Development/mp3player/testdata/generated/media-events.log'
exec '/Users/davidstolarsky/Development/mp3player/target/debug/shuffle' '/Users/davidstolarsky/Development/mp3player/testdata/generated/test-tone.mp3'
