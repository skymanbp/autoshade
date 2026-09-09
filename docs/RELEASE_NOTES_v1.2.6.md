# AutoShade v1.2.6 — a compressed download stops being reported as a truncated one

AI denoise could not fetch its own model on a machine that did not already
have one cached. The button reported

```
AI denoise: denoise sidecar exited with 1: [denoise] device=cuda:0 model=color_real_psnr strength=1.0
[denoise] downloading network_scunet.py ...
[denoise]   0/0 MB (393.6%)
refusing network_scunet.py: the download stopped at 11445 of 2908 bytes
```

and the download it refused was byte-for-byte correct.

## The two numbers being compared were never the same quantity

`python/denoise.py`'s `_download` streams a pinned file, counts what it
writes, and refuses to publish a short transfer onto the cache name — a
server or proxy that closes early used to surface later as a confusing
torch/pickle error instead of the true reason. It charged that check against
the response's `Content-Length`.

`Content-Length` counts the bytes **on the wire**. `requests.iter_content`
hands back the **decoded** body. On an endpoint that compresses, those are two
different numbers, and raw.githubusercontent.com compresses: the pinned
`network_scunet.py` arrives as `Content-Length: 2908` with
`Content-Encoding: gzip`, and decodes to 11445 bytes whose SHA-256 is
`77aeefd31e37080db7f0bf46bca5efcecc800fcfddb502081340a10b2b949c60` — exactly
the pin the file has carried since v0.23.2. The progress line said the same
thing in the same breath: 11445 ÷ 2908 is 393.6 %.

The guard was introduced in `61bbd18` (2026-08-11) and first shipped in
v0.23.2, so every release from then through v1.2.5 could fetch the pinned
`.py` only onto a cache that already held it. On a fresh install there is no
such cache, and both timings of the denoiser — the canvas button and the
export-time toggle — ended at that refusal.

The weights were never affected, and that is not luck: the KAIR release asset
is served uncompressed (`Content-Length: 71982841`, which is the pinned byte
count for `color_real_psnr`), so the comparison was between like and like.

## The fix is a request header, not a relaxed check

`_download` now asks for `Accept-Encoding: identity`, which makes
`Content-Length` mean the same thing as what gets written, and treats a
response that arrives content-encoded anyway as **sizeless** rather than
truncated — zero is the value the rest of the function already reads as "the
endpoint did not tell us a size". The in-stream byte cap is unchanged: it was
always charged in decoded bytes against the pinned tables, which count the
file rather than the transfer. `_fetch_verified`'s SHA-256 remains the
authority in every case.

## The same forty lines are all five sidecars

`_download` is not denoise's alone. `python/_sidecar.py` imports
`denoise._fetch_verified` on behalf of `describe.py`, `embed.py` and
`correspond.py`, and `segment.py` reaches it the same way — which is why every
download line in the family says `[denoise]`. They escaped this defect for a
reason worth writing down rather than assuming: huggingface.co **also** gzips
the pinned JSON files, but sends no `Content-Length` at all, so their
short-download check had nothing to compare and skipped every time.

Asking for `identity` fixes that too. huggingface.co then answers
`Content-Length: 47164` for the pinned `tokenizer_config.json`, which is the
byte count `embed.py` pins for it — so the guard is doing real work on the
model downloads for the first time since it was written.

## What was measured

The pre-fix file reproduces the reported message verbatim under the new tests
(`refusing network_scunet.py: the download stopped at 11445 of 2908 bytes`),
and the fixed one publishes 11445 bytes. `python/test_denoise.py` pins six
contracts: a gzipped response is not a short download, the request asks for no
content encoding, a genuinely short transfer is still refused, a stream past
the pinned size is still refused mid-flight, a refusal leaves no `.part`
behind, and an endpoint that sends no length still publishes.

End to end on a cold cache, running the installed sidecar the way the desktop
app spawns it: exit 0, `network_scunet.py` 11445 bytes and
`scunet_color_real_psnr.pth` 71,982,841 bytes both matching their pinned
digests, no `.part` left behind, and a 16-bit frame whose RMS error against
its clean reference fell from 0.03497 to 0.00331.

## Gates

Release battery, this machine: library **1386 passed / 0 failed / 14 ignored**
(1400 enumerated, one process per module), CLI **24**, integration **2 + 2**,
doc-tests 0, GUI **164 passed / 0 failed**; `clippy --release --all-targets` 0
warnings on the default feature set and 0 with `--features gui`; `audit_i18n` 0
findings across its eleven checks; `subset_gui_fonts --check` 875/875 embedded;
`check_docs.py --gates` **28 PASS / 0 FAIL / 2 SKIP**. The doc gate caught a
fourth copy of the battery counts, in `docs/TECH_STACK.md`, that a hand grep had
missed. By-name test-set difference: **+1 / −0** —
`denoise::the_sidecar_downloader_asks_for_an_unencoded_body`. The saved name
baseline was deleted with `target/` in the 2026-09-03 clean-up, so that
difference was taken directly between the v1.2.5 tag's source and this tree
rather than against a stored list.

The Python suites — `test_denoise` 6, `test_sidecar` 13, `test_segment` 6, all
passing — are a local gate only: no workflow runs them. That is why the
`identity` request is *also* pinned from Rust, in
`denoise::tests::the_sidecar_downloader_asks_for_an_unencoded_body`, which rides
the library suite into CI. Both of its assertions were falsified by hand — the
header removed, then the sizeless-on-encoded arm removed — and each went red
by name before the file was restored to a byte-identical SHA-256.

**The calibration lane did not run, and is not claimed**, for the same reason as
in v1.2.5: its p36–p39 fit corpus was deleted in the 2026-09-03 clean-up, and
`scripts/release_battery.sh` exits 1 rather than let corpus-gated tests skip and
pass. Its subject is the fit estimator, which this release does not touch. Two
consequences are disclosed rather than papered over: the transcript here was
produced by running the script's own two remaining lanes directly, so
`check_docs.py` reports its lane check as SKIP ("no `=== test calib ===` block —
it was not written by `release_battery.sh`"), and the active-XMP census SKIPs
because its corpus lives outside the repository.
