# graybox

A Nintendo Entertainment System emulator, built one chapter at a time — the companion
code for Part I of a book in progress about writing a NES emulator in Rust from scratch.

Each tag is the complete state of the code at the end of a chapter, and every one of
them compiles and passes its tests:

```
git checkout ch15    # the chapter where a real game first paints its title screen
cargo test
cargo run roms/Chase.nes
```

ROMs are not included. The games this emulator grows up alongside — Chase, Lan Master,
Zooming Secretary and friends — are free downloads from
[Shiru's site](https://shiru.untergrund.net/software.shtml); put them in `roms/`.
