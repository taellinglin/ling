# 道場 — 社の部屋

ソース：[`examples/3d-rooms/ling-dao-chamber.ling`](https://github.com/taellinglin/ling/blob/master/examples/3d-rooms/ling-dao-chamber.ling)

ベクタージオメトリ（`vtex_*`）だけで描いた、静かな中国・日本風の社です。ピクセルの塗りつぶしは使っていません。
すべてが金・翡翠・深紅の配色でゆっくりと動き、背景では五音音階の鐘がやさしく鳴ります。プログラムは主にタイ語の
キーワードと組み込み関数で書かれていて、中国語の名前もいくつか混ざっています。

リポジトリのルートで実行します。

```sh
ling run examples/3d-rooms/ling-dao-chamber.ling
```

## 部屋にあるもの

| 部分 | 使っている組み込み関数 |
|---|---|
| 奥の鳥居 | `vtex_torii` |
| 鳥居の後ろの五重塔 | `vtex_pagoda` |
| 祭壇の曼荼羅：歯車の輪、法輪、蓮、輪 | `vtex_spiked_cog`、`vtex_chakra`、`vtex_lotus`、`vtex_rings` |
| ヤントラの縁取りがある石畳の床 | `vtex_grid`、`vtex_yantra` |
| 天井の星形の灯籠 | `vtex_star`、`vtex_rings` |
| 左右の壁の墨絵パネル | `vtex_letter_rain`、`vtex_lotus` |
| 四隅の柱 | `vtex_rings`、`vtex_spiked_cog`、`vtex_lotus` |

## サウンド

5 つの音で中国の五音音階 宫 商 角 徵 羽（C4、D4、E4、G4、A4）を作り、さらにかすかな低音を加えています。
それぞれの音は `audio_tone` で 3D 空間に置かれ、`audio_listener` がカメラについていくので、視点が揺れると鐘の聞こえ方も
変わります。

## カメラとライト

カメラはゆっくりした振り子のように左右へ約 30 度ずつ揺れ、いつも祭壇のほうを向いています。5 つのライト（`add_light`）が、
天井の金色のランプ、祭壇の暖かい光、柱の翡翠と深紅のアクセント、鳥居のランプを作ります。

## 試してみよう

- ファイル先頭の色相の基準 `GOLD`、`JADE`、`CRIMSON` を変えてみましょう。
- `orbit_spd` を大きくして、カメラを速く動かしてみましょう。
- `init_audio` の周波数を変えて、別の音階にしてみましょう。

ここで使っている組み込み関数については、[3D 形状とメッシュ](../reference/shapes.md)と[オーディオ](../reference/audio.md)を
見てください。
