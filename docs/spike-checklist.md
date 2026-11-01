# Raven spike: checklist de go / no-go (PT-BR) / go / no-go checklist (EN)

Spec §9, item 1. O resultado decide se o Raven (Vulkan) serve nos PCs de quem vai usar o SaveStates.
The result decides whether Raven (Vulkan) is acceptable on the target users' PCs.

Os binários e o spike vêm do commit "build: workspace, pinned Raven v0.2.0, Raven spike".
The binaries and the spike come from the commit "build: workspace, pinned Raven v0.2.0, Raven spike".

## Como rodar / How to run

PT-BR:

1. Instale Rust 1.96 (rustup) e o Visual Studio Build Tools (MSVC). Instale o Git.
2. `pwsh scripts/fetch-raven.ps1` baixa e confere (SHA256) o `raven.lib`.
3. `cargo run -p savestates-ui-raven --example spike --release`
4. Abre uma janela 1100×420 com 10.000 "saves" ("Save #n") numa linha. Arrastar com o botão esquerdo move, Ctrl+roda do mouse dá zoom em volta do cursor, a roda sozinha rola na horizontal. O título da janela mostra FPS, zoom, quantas estações foram desenhadas e o tempo de pintura.
5. O FPS só conta quando a tela está sendo repintada: arraste ou dê zoom continuamente enquanto lê o título (parado, aparece ~1 FPS).

EN:

1. Install Rust 1.96 (rustup), the Visual Studio Build Tools (MSVC) and Git.
2. `pwsh scripts/fetch-raven.ps1` downloads and verifies (SHA256) `raven.lib`.
3. `cargo run -p savestates-ui-raven --example spike --release`
4. A 1100×420 window opens with 10,000 "saves" ("Save #n") on one line. Left-drag pans, Ctrl+wheel zooms around the cursor, the plain wheel scrolls horizontally. The window title shows FPS, zoom, how many stations were drawn and the paint time.
5. FPS only counts while the screen repaints: keep dragging or zooming while you read the title (idle shows about 1 FPS).

Opcional / Optional: `spike.exe --selftest <prefixo>` injeta arrasto e Ctrl+roda sozinho, imprime PASS/FAIL e grava `<prefixo>-zoom1.bmp` e `<prefixo>-zoom2_5.bmp` (captura do quadro da GPU). / injects a drag and a Ctrl+wheel by itself, prints PASS/FAIL and writes the two BMP screenshots.

## Máquinas / Machines

Preencha uma seção por máquina. / Fill in one section per machine.

### (a) Notebook com vídeo integrado / Laptop with integrated graphics

- [ ] A janela abre / The window opens
- [ ] Arrastar é fluido, ≥ 50 FPS (veja o título) / Dragging is smooth, ≥ 50 FPS (see the title)
- [ ] Zoom com Ctrl+roda é fluido / Ctrl+wheel zoom is smooth
- [ ] Os textos ficam nítidos no zoom 1,0 e no zoom 2,5 / Labels are crisp at zoom 1.0 and 2.5
- GPU e versão do driver / GPU and driver version: ____________________
- Observações / Notes: ____________________

**Go / No-go:** [ ] GO  [ ] NO-GO

### (b) Desktop com placa de vídeo dedicada / Desktop with a discrete GPU

- [ ] A janela abre / The window opens
- [ ] Arrastar é fluido, ≥ 50 FPS (veja o título) / Dragging is smooth, ≥ 50 FPS (see the title)
- [ ] Zoom com Ctrl+roda é fluido / Ctrl+wheel zoom is smooth
- [ ] Os textos ficam nítidos no zoom 1,0 e no zoom 2,5 / Labels are crisp at zoom 1.0 and 2.5
- GPU e versão do driver / GPU and driver version: ____________________
- Observações / Notes: ____________________

**Go / No-go:** [ ] GO  [ ] NO-GO

### (c) PC mais antigo de um amigo (se houver) / An older friend's PC (if available)

- [ ] A janela abre / The window opens
- [ ] Arrastar é fluido, ≥ 50 FPS (veja o título) / Dragging is smooth, ≥ 50 FPS (see the title)
- [ ] Zoom com Ctrl+roda é fluido / Ctrl+wheel zoom is smooth
- [ ] Os textos ficam nítidos no zoom 1,0 e no zoom 2,5 / Labels are crisp at zoom 1.0 and 2.5
- GPU e versão do driver / GPU and driver version: ____________________
- Observações / Notes: ____________________

**Go / No-go:** [ ] GO  [ ] NO-GO

## Decisão final / Final decision

- (a) Notebook integrado / Laptop integrated: [ ] GO  [ ] NO-GO
- (b) Desktop dedicada / Desktop discrete: [ ] GO  [ ] NO-GO
- (c) PC antigo / Older PC: [ ] GO  [ ] NO-GO

Se qualquer máquina alvo der NO-GO, pare e avise o dono do produto. Não troque o Raven por conta própria (spec §9).
If any target machine is NO-GO, stop and tell the product owner. Do not replace Raven on your own (spec §9).

## Registro técnico do build / Build log

- Raven v0.2.0, `raven-windows-msvc-x64-static.zip`, SHA256 `774cb1fe9382750a0b7a310c23e8e0a82357ea6b9d0914cec6c6775b62d2011f`. Link estático funcionou / Static linking worked (no switch to the shared zip).
- O `build.rs` do binding vendorizado não lista a `winhttp`, que o `raven.lib` precisa (`rvn_fs_download_url`). `crates/ui-raven/build.rs` adiciona `winhttp` (a mesma lista do pacote CMake do Raven). / The vendored binding's `build.rs` does not list `winhttp`, which `raven.lib` needs. `crates/ui-raven/build.rs` adds it (same list as Raven's CMake package).
- Cargo só aplica o `rustc-link-lib` do `build.rs` quando o rlib do `ui-raven` entra no link; o exemplo faz `extern crate savestates_ui_raven as _;` por isso. O `app` usará o `ui-raven` de verdade. / Cargo only applies that `rustc-link-lib` when the `ui-raven` rlib is linked, which is why the example does `extern crate savestates_ui_raven as _;`. The real `app` will use `ui-raven` normally.
- Verificação automática na máquina de desenvolvimento (não substitui as máquinas acima) / Automated check on the dev machine (does not replace the machines above): NVIDIA GeForce RTX 5080, driver 32.0.16.2101, monitor 60 Hz. `--selftest`: janela abriu, arrasto de 3 s repintou a ~62 FPS (limitado pelo vsync de 60 Hz), zoom 1,0 → 2,5 manteve o ponto sob o cursor, capturas com texto nítido. / window opened, a 3 s drag repainted at ~62 FPS (vsync-capped at 60 Hz), zoom 1.0 → 2.5 kept the point under the cursor, screenshots show crisp text.
