# 音響ボディ

`body_*` 組み込み関数は `ling-body` を公開します。音から人型の皮膚を育てるツールキットです。ボディの骨組み（パラメトリックな T ポーズの人物、または読み込んだモデル）を符号付き距離場にボクセル化します。ボディに置いた音源が、その内外の圧力波を励起します。薄い弾性膜が波に反応し、その表面が水密な三角形メッシュとして取り出されます。

このレシピは WAV ファイルを往復できます。書き出した WAV は音として再生でき、同時にパラメーターとメッシュも運ぶので、読み込めば同じボディに戻ります。ほかの音声ファイルを解析して音源のパラメーターにすることもできます。

知っておくこと：

- ツールキットはプログラムに 1 つだけで、`body_*` の呼び出しはすべてそれを操作します。
- 失敗しうる操作は、成功で `1`、失敗で `0` を返し、メッセージを残します。メッセージは `body_message()` で読みます。
- 値は数値、文字列、平たいリストとしてやり取りします。
- 単位はメートルです。メッシュは Y 軸が上で、人物は +Z を向き、体の左側が +X です。
- `body_draw` と `body_draw_creases` 以外は、ウィンドウなしで動きます。
- これらの組み込み関数にはローカライズ名がありません。

## 最初のボディ

いくつかのパラメーターを設定し、音源を追加し、シミュレーションを少しずつ進めてから、メッシュを読んで書き出します。

```ling
束縛 開始 = 実行 {
    body_param_set("sr", 8000)
    body_param_set("steps", 60)
    body_param_set("seal", 1)
    body_src_add(0.0, 1.0, 0.0, 0, 220)
    印刷("ステップ数: " + str(body_sim_begin()))

    束縛 進捗 = [0, 1]
    間 進捗[0] < 進捗[1] {
        束縛 進捗 = body_sim_step(40)
    }
    印刷("完了: " + str(body_sim_done()))
    印刷("メッシュ: " + str(body_mesh_update()) + " " + body_message())
    束縛 個数 = body_mesh_counts()
    印刷("水密: " + str(個数[2]))
    印刷(body_grid_info())
    印刷("書き出し: " + str(body_export_mesh("body.obj")) + " " + body_message())
}
```

```output
ステップ数: 60
完了: 1
メッシュ: 1 mesh updated
水密: 1
109x107x75 @ 41.3 mm
書き出し: 1 10008 triangles → body.obj
```

## 一覧

| 組み込み関数 | 用途 |
|---|---|
| `body_message()` | 直前の操作のメッセージ |
| `body_grid_info()` | `"109x107x75 @ 41.3 mm"` のようなグリッドの大きさと間隔 |
| `body_notes()` | 直前のリスニングや WAV 解析のメモ |
| `body_param_get(name)` / `body_param_set(name, v)` | パラメーターを読む・設定する |
| `body_param_reset()` | すべてのパラメーターを戻し、音源をすべて削除 |
| `body_param_count()`, `body_param_name(i)`, `body_param_group(i)` | パラメーターを列挙 |
| `body_param_min(i)`, `body_param_max(i)`, `body_param_default(i)` | パラメーターの範囲と既定値 |
| `body_src_add(x, y, z, wave, freq)` | 位置を指定して音源を追加 |
| `body_src_at_joint(joint, wave, freq)` | 骨組みのジョイントに音源を追加 |
| `body_src_count()`, `body_src_name(i)`, `body_src_del(i)`, `body_src_clear()` | 音源を管理 |
| `body_src_get(i, field)` / `body_src_set(i, field, v)` | 音源のフィールドを読む・変える |
| `body_joint_count()`, `body_joint_name(i)`, `body_joint_pos(i)`, `body_joint_parent(i)` | 骨組みのジョイント |
| `body_model_load(path)`, `body_model_use(h)`, `body_model_active()` | 読み込んだモデルをボディとして使う |
| `body_model_part_count(h)`, `body_model_part_name(h, i)`, `body_model_part_tris(h, i)` | モデルのパーツを調べる |
| `body_model_part_enabled(h, i)`, `body_model_part_set(h, i, on)` | パーツを含める・外す |
| `body_preview()` | シミュレーションせずにボクセル化して静止形状を表示 |
| `body_sim_begin()`, `body_sim_step(ms)` | シミュレーションを開始・進める |
| `body_sim_active()`, `body_sim_done()` | シミュレーションの状態 |
| `body_mesh_update()` | 膜を作業用メッシュとして取り出す |
| `body_displacement()`, `body_slice(axis, frac)`, `body_mic_tail(n)` | シミュレーションを調べる |
| `body_mesh_counts()`, `body_mesh_bounds()` | メッシュの統計 |
| `body_mesh_positions()`, `body_mesh_normals()`, `body_mesh_indices()` | メッシュの生データ |
| `body_crease_detect()`, `body_crease_apply()`, `body_detail_reset()` | 音場からしわを刻む |
| `body_crease_count()`, `body_crease_segments()`, `body_crease_clear()`, `body_crease_export(path)` | しわの曲線を扱う |
| `body_listen_begin()`, `body_listen_step(ms)`, `body_listen_done()`, `body_listen_finish(path)` | ボディの音響シグネチャを録る |
| `body_listen_spectrum()` | シグネチャの対数間隔の 48 帯 |
| `body_export_mesh(path)`, `body_export_wav(path)`, `body_import_wav(path, analyze_only)` | ファイル |
| `body_project_save(path)`, `body_project_load(path)` | レシピを JSON で保存・読み込み |
| `body_draw(mode, yaw, x, y, z, scale, extra)`, `body_draw_creases(yaw, x, y, z, scale)` | ウィンドウに描画 |

## パラメーター

パラメーターは、名前の付いた 47 個の数値からなる固定の表です。それぞれにグループ、既定値、範囲があり、値を設定すると範囲内にクランプされます。`body_param_set` は知っている名前なら `1`、そうでなければ `0` を返します。列挙用の組み込み関数は、0〜46 の番号か名前を受け取ります。

| グループ | 名前（既定値） | 意味 |
|---|---|---|
| body | `height` (1.7), `build` (1), `arm_span` (1), `head_scale` (1) | 骨組みの比率。`height` は読み込んだモデルの大きさにも効きます |
| body | `seal` (3) | 読み込んだモデルの開口部をふさぐボクセル半径 |
| sim | `sr` (22050), `steps` (800), `margin` (16), `up` (2) | サンプルレート、時間ステップ数、空気の余白（セル数）、膜グリッドの細分化 |
| medium | `c_air` (343), `c_body` (1), `rho_body` (4), `damping` (0.002), `sponge` (8) | 音速、ボディの音速比と密度比、損失、吸収層 |
| mic | `mic_sep` (0.18), `mic_dist` (0.15) | 耳の間隔と、顔の前の距離 |
| membrane | `relief`, `coupling`, `threshold`, `tension`, `stiffness`, `mem_time`, `mem_every`, `reinit_every`, `band`, `drive_mode`, `drive_tau` | 皮膚が波にどう追従するか |
| mesh | `smooth` (3), `decimate` (0), `iso` (0) | スムージングの回数、頂点を結合するセルの大きさ（メートル）、表面レベルのずらし |
| audio | `wav_bits` (16), `wav_gain` (0.9), `wav_repeat` (4), `wav_gap` (0.25) | WAV の書き出し |
| detail | `crease_mode`, `crease_levels`, `crease_tol`, `crease_min`, `detail_level`, `crease_depth`, `crease_width`, `noise_amp`, `noise_freq` | しわの検出と彫り込み |
| listen | `sig_dur`, `sig_sr`, `sig_rho`, `sig_excite` | リスニングの処理 |

```ling
束縛 開始 = 実行 {
    印刷(body_param_count())
    印刷(body_param_get("height"))
    印刷(body_param_set("height", 5))
    印刷(body_param_get("height"))
    印刷(body_param_set("nope", 1))
    印刷(body_joint_count())
    印刷(body_joint_name(4))
}
```

```output
47
1.7
1
2.4
0
21
head
```

`height` の上限は 2.4 なので、5 を設定すると 2.4 が入ります。

## 音源とジョイント

音源は空間に置いた励起です。フィールドは `x`、`y`、`z`（メートル）、`wave`（`0` リッカーウェーブレット、`1` サインバースト、`2` インパルス）、`freq`（Hz）、`amp`、`phase`、`start`、`dur`（秒）です。

- `body_src_add(x, y, z, wave, freq)` は音源を 1 つ追加し（既定値 0, 1, 0、wave 0、220 Hz）、その番号を返します。
- `body_src_at_joint(joint, wave, freq)` は名前を指定したジョイントに音源を追加し、音源にそのジョイントの名前を付けます。知らないジョイントなら `-1` を返します。

骨組みには 21 個のジョイントがあります。`hips`、`spine`、`chest`、`neck`、`head`、そして `shoulder`、`elbow`、`wrist`、`hand`、`hip`、`knee`、`ankle`、`toe` の `l_` / `r_` 版です。`body_joint_pos(i)` は現在のボディの大きさに合わせた `[x, y, z]` を返し、`body_joint_parent(i)` は親の番号（ルートなら `-1`）を返します。

## 読み込んだモデル

`body_model_load(path)` は `.glb`、`.gltf`、`.lmesh`、`.obj` のファイルを読み込み、ハンドル（または `-1`）を返します。読み込んだだけでは選ばれないので、`body_model_use(h)` を呼んでください。負のハンドルを渡すと骨組みに戻ります。glTF ファイルはジオメトリとノードの変換だけを読むので、テクスチャ拡張が必要なファイルも読み込めます。花や立方体のようなアクセサリーのパーツは最初は無効です。パーツの切り替えには `body_model_part_set(h, i, on)` を使います。

## シミュレーション

シミュレーションは時間を区切って少しずつ進むので、ウィンドウは更新を続けられます。

- `body_preview()` はボディをボクセル化し、シミュレーションせずに乱れのない形を取り出します。
- `body_sim_begin()` はシミュレーションを開始し、全ステップ数か `-1` を返します。
- `body_sim_step(ms)` は約 `ms` ミリ秒（既定値 30）だけ処理し、`[済んだステップ数, 全体]` を返します。
- `body_mesh_update()` は今の膜を取り出します。シミュレーション中に呼んで、皮膚が育つ様子を見ることもできます。
- `body_displacement()` は `[内向きmm, 外向きmm]` を返します。`body_slice(axis, frac)` は -1..1 に正規化した圧力場の断面 `[w, h, v0, v1, ...]` を返します。`body_mic_tail(n)` は 1 つ目のマイクの最後の `n` サンプルを返します。

## しわとディテール

シミュレーションが終わったあと、`body_crease_detect()` はメッシュ上で音場をサンプリングし、その等高線をたどって 3 次ベジェ曲線で近似します。`body_crease_apply()` はメッシュを細分化し、すべての曲線に沿って柔らかな溝を刻み、音場で重み付けした細かなノイズも加えます。`body_detail_reset()` は滑らかなメッシュに戻します。`body_crease_export(path)` は曲線を `.json` として、または曲線オブジェクトを作る Blender 用の `.py` スクリプトとして書き出します。

## リスニング

`body_listen_begin()` は口の前でインパルスを鳴らし、頭の横にある 2 つのマイクで録音します。終わるまで `body_listen_step(ms)` を呼び（`[済み, 全体]` を返します）、そのあと `body_listen_finish(path)` でボディの音を WAV に書き出し、`body_listen_spectrum()` と `body_notes()`（最も強い共鳴）を埋めます。

## ファイル

- `body_export_mesh(path)` は拡張子に応じて OBJ、STL、GLB を書き出します。GLB には T ポーズのスケルトンとスキンウェイトが入ります。
- `body_export_wav(path)` はステレオのマイク信号を書き出し、レシピ全体と圧縮したメッシュを持つ 2 つの追加チャンクを付けます。
- `body_import_wav(path, analyze_only)` は、チャンクがあれば両方を復元します。`analyze_only` が 1 のとき、またはチャンクがないときは、代わりに音声から音源を推定し、`body_notes()` がそれぞれの推定を説明します。WAV、FLAC、Ogg、MP3 を受け付けます。
- `body_project_save(path)` と `body_project_load(path)` は、パラメーター、音源、選んだボディを JSON で保存・読み込みします。

## 描画

`body_draw(mode, yaw, x, y, z, scale, extra)` は作業用メッシュをペンの色で描きます。レンダラーの y 下向きの空間へ反転されます。モード：

| モード | 見た目 |
|---|---|
| 0 | ソリッド、滑らかな法線 |
| 1 | `extra` 個おきの三角形のワイヤーフレーム |
| 2 | 皮膚の変位で色付け。`extra` はフルスケールの押し出し量（ミリ、既定値 10） |
| 3 | 面の法線で色付け |
| 4 | 1 灯のクレイ風シェーディング |

モード 3 と 4 は細かな起伏を見分けやすくします。`body_draw_creases(yaw, x, y, z, scale)` は検出したしわの曲線を上に重ねて描きます。

このウィンドウでは、見ている間にボディが育ちます。

```ling-gui
束縛 開始 = 実行 {
    ウィンドウ開く(640, 480, "ボディ")
    body_param_set("steps", 400)
    body_src_at_joint("chest", 1, 180)
    body_sim_begin()
    束縛 向き = 0.0
    間 開いている() {
        もし body_sim_done() == 0 {
            body_sim_step(20)
            body_mesh_update()
        }
        塗り潰し(16, 16, 24)
        色設定(220, 200, 180)
        body_draw(2, 向き, 0, 1, 4, 1, 10)
        flush_3d()
        表示()
        束縛 向き = 向き + 0.01
    }
}
```

関連ページ：[物理](physics.md)、[音楽](music.md)
