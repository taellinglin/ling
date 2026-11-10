# ショーケース

ここで紹介するアプリとデモは、どれも Ling で書かれ、Ling のランタイムで動いています。アプリのスクリーンショットは
Web サイトで見られます：[ling-lang.org/#apps](https://ling-lang.org/#apps)。

## アプリ

| アプリ | 概要 |
|---|---|
| **LingStationX** | デジタルオーディオワークステーション。アレンジャー、ピアノロール、トラッカー、セッションビュー、ノードグラフのパッチャー、ミキサー、そして Nova・Prism・Growl シンセを備えています。MIDI の読み込み、スライサー、タイムワープ、マスタリングチェーンもあります。 |
| **LingDev** | ノードツリー、タイムライン、カメラ・ダイアログ・エフェクト用のカットシーントラックを持つ 3D ゲームエディター。Dance Ling、Ling Hazard、Ling Breaker、Moko、Ling Agent、Soul Symphony シリーズはこれで作られています。 |
| **LingModeler** | モデリング、リギング、ウェイトペイント、ポーズ付け、アニメーションに加え、ベクターイラスト、オニオンスキン付きの 2D アニメーション、プロシージャルテクスチャ。x86-64 と Raspberry Pi の LingOS 上でも動きます。 |
| **LingForFun** | フィールド、町、ダンジョン、飛行、戦闘、クエストのあるオンラインアクション RPG。デスクトップクライアント、WASM ブラウザクライアント、サーバークラスターをすべて Ling で書いています。中国語のキーワードで書かれています。開発中です。 |
| **Ling Arcade** | Ling で書かれた NES エミュレーターを内蔵したゲームライブラリ。6502 CPU、PPU、マッパー、APU はすべて Ling の JIT 上で動く普通の `.ling` コードです。nestest のログを最後まで通過します（8,991 命令、サイクル精度）。 |
| **Ling Wallet** | LNG 用の耐量子プライバシーウォレットで、フルノードとマイナーも兼ねます。ML-DSA-87 の ID と、ML-KEM-1024 + X25519 のハイブリッド鍵交換を使います。実験的なもので、監査は受けていません。 |

## コミュニティのプロジェクト

| プロジェクト | 概要 |
|---|---|
| [**Soul Symphony**](https://github.com/taellinglin/soul-symphony-ling-edition) | RPG とアーケードと物理を組み合わせたゲーム。オーブを転がして 9 つのプロシージャル生成ステージを進み、各ステージの最後にボス戦があります。アーケード対戦、ネットワーク対応、協力プレイがあり、5 つの言語で遊べます。 |
| [**LingSoul**](https://github.com/taellinglin/LingSoul) | Ling のワイヤーフレーム風に描かれた、雰囲気のある魂のバランスシミュレーション。ゲーム内の HUD には独自に作った文字体系を使っています。 |

## リポジトリのデモ

[Ling のリポジトリ](https://github.com/taellinglin/ling)には、すぐに動かせる例がたくさんあります。まずはこのあたりから
試してみてください。

| デモ | 内容 |
|---|---|
| `examples/music/hallway_runner.ling` | 音楽に反応するエンドレスランナー。コインが空間オーディオでメロディを奏でます |
| `examples/music/karaoke_hero.ling` | ボーカル MIDI から作るカラオケの音程バーと、ライブのマイクチューナー |
| `examples/music/synth_jam.ling` | `.ling` のパッチから合成する GM 楽器 |
| `examples/physics/bouncy_ball.ling` | 弾むたびにつぶれるばね質点のソフトボディ |
| `examples/physics/tumbling.ling` | 転がり回る剛体 |
| `examples/physics/liquid_marble.ling` | 回転する球、円錐、ドームの上での水と油のシミュレーション |
| `examples/ui/hud_showcase.ling` | UI ツールキット：HUD、メーター、コントロール、ゲーム用ウィジェット |
| `examples/crypto/crypto_donut_physics.ling` | SHA3 ハッシュで形が決まる弾むトーラス結び目と、それに反応する音 |

リポジトリのルートで 1 つ動かしてみましょう。

```sh
ling run examples/physics/liquid_marble.ling
```

このドキュメントの[サンプル](examples/dao-chamber.md)の章では、4 つの 3D シーンをさらに詳しく紹介しています。
