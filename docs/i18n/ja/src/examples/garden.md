# 庭園

ソース：[`examples/audiovisual/Garden.ling`](https://github.com/taellinglin/ling/blob/master/examples/audiovisual/Garden.ling)

開けた屋外の庭です。地面と空、中央の池、手前の 2 本の木、花壇、奥の木立、木の小屋があり、壁や天井はありません。
すべてをベクタージオメトリ（`vtex_*`）で描き、形の種類ごとに音が割り当てられているので、見るだけでなく聞くこともできる
シーンになっています。

リポジトリのルートで実行します。

```sh
ling run examples/audiovisual/Garden.ling
```

## シーン

| 部分 | ファイル内の関数 |
|---|---|
| 草地、石の小道、花壇の区画 | `draw_ground` |
| 太陽、流れる雲、地平線の光 | `draw_sky` |
| らせんの幹と花の樹冠を持つ木 | `draw_tree`、`draw_trees` |
| あちこちの花壇 | `draw_flowers` |
| 蓮が浮かび、噴水と波紋のある池 | `draw_pond` |
| 扉、窓、屋根のある木の小屋 | `draw_shed` |
| 少し前のフレームから作る、うっすらとした 2 つの「残像」 | `draw_echo` |

形には `vtex_flower`、`vtex_lotus`、`vtex_chakra`、`vtex_rings`、`vtex_grid`、`vtex_spiral`、`vtex_star`、
`vtex_yantra`、`vtex_hyperbolic_uv`、`vtex_tessellated` を使っています。小さなハッシュ関数 `sm2_hash` によって、
地面の区画や小屋の板ごとに細かさが変わります。

## サウンド

6 つのトーンの声部が、ト長調の五音音階（G A B D E）を使います。各声部は `audio_tone` でシーン内のそれぞれの位置
（池、小屋、空のまわり）に置かれ、`audio_listener` がカメラについていきます。

## カメラ

カメラは池のまわりを、ゆっくりと気ままな軌道で漂い、いつも庭の中心を見ています。色はおよそ 2 秒ごとに全色相を
ひと巡りします。

## 試してみよう

- `orbit_h` と `orbit_v` を変えて、カメラを近づけたり遠ざけたりしてみましょう。
- `PX` と `PZ` で池を、`SX` と `SZ` で小屋を動かしてみましょう。
- 五音音階の周波数 `PG3` から `PE4` を変えて、庭の調を変えてみましょう。
