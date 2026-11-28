<p align="center">
  <a href="https://getartcraft.com/">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="docs/brand/artcraft-logo-white.svg">
      <img alt="ArtCraft" src="docs/brand/artcraft-logo.svg" width="200">
    </picture>
  </a>
</p>


<p align="center">
  <img src="assets/app-icon/hicolor/256x256/apps/ai.storyteller.effectcraft.png" width="128" height="128" alt="EffectCraft のアイコン: ピンクのタイルに刻まれたユニコーン">
</p>

<h1 align="center">EffectCraft</h1>

<p align="center">
  <b>モーショングラフィックスと視覚効果。純Rustで作られた、オープンソースのプロ向けモーショングラフィックス＆VFXスタジオ。</b>
</p>

<p align="center">
  無料のオープンソース・コンポジター。コンポジション、レイヤー、キーフレーム、308個のエフェクト、
  レイヤースタイル、エクスプレッション、3Dカメラとライト、レンダーキューを備え、macOS・Windows・Linuxで
  ネイティブに、そしてブラウザでも動きます。若く、開発が速く、すでに実用段階にあります。
</p>

<p align="center">
  <img alt="ステータス: 若く、開発が速い" src="https://img.shields.io/badge/status-young%20and%20moving%20fast-e0368f?style=flat-square">
  <img alt="Rust で書かれている" src="https://img.shields.io/badge/rust-1.95%2B-b0206c?style=flat-square&logo=rust&logoColor=white">
  <img alt="ライセンス: MIT または Apache-2.0" src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-555?style=flat-square">
  <br>
  <a href="https://sonarcloud.io/project/overview?id=storytold_effectcraft"><img alt="SonarCloud でスキャン済み" src="https://img.shields.io/badge/Scanned_by-SonarCloud-4b9fd5?style=flat-square&logo=sonarqubecloud&logoColor=white"></a>
  <a href="https://coderabbit.ai"><img alt="CodeRabbit でレビュー済み" src="https://img.shields.io/coderabbit/prs/github/storytold/effectcraft?style=flat-square&label=Reviewed%20by%20CodeRabbit&logo=coderabbit"></a>
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="Discord の ArtCraft コミュニティに参加" src="https://img.shields.io/badge/Join%20us%20on%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<p align="center">
  <a href="https://getartcraft.com/apps/effectcraft"><b>getartcraft.com の EffectCraft</b></a> ·
  <a href="https://getartcraft.com/">ArtCraft</a> ·
  <a href="https://getartcraft.com/apps">すべての Crafting Apps</a>
</p>

<br>

<p align="center">
  <img src="docs/images/effectcraft-hero.png" alt="EffectCraft のメインウィンドウ: コンポジションパネルのアニメーションデモ、プロジェクトパネル、テキスト・シェイプ・ソリッドレイヤーのあるタイムライン、選択中のテキストレイヤーのトランスフォーム・フォント・段落設定を示すプロパティパネル" width="100%">
</p>

> [!NOTE]
> **ArtCraft は、あらゆる背景を持つアーティストが集うコミュニティです。** デジタル、ジェネラティブ、音楽、
> ゲーム &mdash; 何かを作っているなら、あなたも仲間です。**[Discord で気軽に話しかけてください](https://discord.gg/artcraft)。**

<p align="center">
  <a href="#effectcraft-とは">EffectCraft とは</a> ·
  <a href="#アニメーション">アニメーション</a> ·
  <a href="#エフェクト">エフェクト</a> ·
  <a href="#3d">3D</a> ·
  <a href="#書き出し">書き出し</a> ·
  <a href="#エージェント向け">エージェント向け</a> ·
  <a href="#はじめかた">はじめかた</a> ·
  <a href="#現状">現状</a> ·
  <a href="#作りかた">作りかた</a> ·
  <a href="#ダウンロード">ダウンロード</a> ·
  <a href="#crafting-apps">Crafting Apps</a> ·
  <a href="#ライセンスとクレジット">ライセンス</a>
</p>

## EffectCraft とは

EffectCraft は、アニメーションタイトル、モーショングラフィックス、コンポジット作業のためのものです。
モーションデザイナーや VFX アーティストが必要とするような作業です。レイヤー（ソリッド、シェイプ、
テキスト、フッテージ、ほかのコンポジション）からコンポジションを組み、キーフレームでプロパティを
アニメーションさせ、エフェクトを重ね、結果をレンダリングします。

インターフェースは自然で反応が良く、専用のパネル（プロジェクト、コンポジション、タイムライン、
レイヤー、フッテージ、エフェクトコントロール、ツール、プレビュー、エフェクト＆プリセット、オーディオ、
情報、段落、文字、整列、進行状況、レンダーキュー）と標準的なモーショングラフィックスのショートカットを
備え、私たちが重視するいくつかの点ではさらに進んでいます:

- **Lottie の読み込みと書き出しを標準搭載**。プラグインなしで Web やアプリへ
  直接持ち込めます。
- **人間が読めるプロジェクトファイル。** プロジェクトはバージョン付きの JSON（`.ecproj`）なので、
  バージョン管理上できれいに差分が取れます。
- **すべてがスクリプト可能。** メニューの操作、タイムラインの編集、プロパティの変更はコマンドとして
  公開されています。エンジンのコマンドは CLI、JSON 制御チャンネル、MCP から使えます。UI 専用の
  コマンドはフロントエンドのディスパッチャを使い、制御チャンネルか接続した MCP ブリッジ経由で
  呼び出せます。
- **FFmpeg を使っていません。** 映像と音声のデコード・エンコードはすべて純Rust: FilmCraft のコーデックと、
  EffectCraft 自身の VP9・AV1・HEVC・Opus エンコーダです。

## アニメーション

パネル、メニュー、ショートカットは標準的なモーションデザインの慣習に従います:
プロジェクト、コンポジション、タイムライン、エフェクトコントロール、プロパティ、エフェクト＆プリセット、
文字、段落、整列、情報、プレビュー、オーディオ、そしてレンダーキューが、おなじみの配置でドッキングできます。
フッテージ、コンポ、エフェクトを相互にドラッグできます: コンポのビューアへ落とせばポインターの位置に、
タイムラインへ落とせばレイヤーとレイヤーの間の、ポインターが指している時刻に入ります。

- **あらゆる種類のレイヤー:** ソリッド、シェイプ、テキスト、フッテージ、ネストしたコンポジション、ヌルオブジェクト、
  調整レイヤー、カメラ、ライト。親子付け、トラックマット、全38種の描画モード、モーションブラー。
- **おなじみの挙動をするキーフレーム:** リニア、ベジェ、ホールド、自動ベジェ、連続ベジェ、ロービングキー、
  イージーイーズ（F9）、キーフレーム速度と補間のダイアログ、キーフレームを現在時刻にコピー＆ペースト、
  値グラフと速度グラフ、ドラッグできるハンドルを備えた**グラフエディター**。
- **時間:** 時間リマップ、時間伸縮、時間反転、フレームを固定、作業領域、マーカー、
  29.97ドロップフレームを含むあらゆるフレームレートでの正確なフレーム単位のタイミング。
- **シェイプとマスク:** パスのトリム、リピーター、角丸、オフセット、ジグザグ、ツイスト、ウィグル、
  パスの合成、グラデーションストロークを備えたシェイプレイヤー。ペンツールで描くマスクは、モード、
  ぼかし、拡張、ビューア上での頂点編集に対応します。
- **テキスト:** ポイントテキストと段落テキスト。マシンにインストールされた任意のフォントで実際に
  シェーピングし、文字パネルと段落パネル、範囲セレクター付きのテキストアニメーターを備えます。
- **エクスプレッション:** JavaScript と標準的なモーションエクスプレッションのモデル（`wiggle`、
  `loopOut`、`thisComp.layer("…")`、配列のベクトル演算）、インラインエディター、ピックウィップ。
- **使い慣れたタイムライン:** レイヤーを開くとトランスフォーム、マスク、エフェクトなどが現れ、行を
  ドラッグして並べ替え、Enter かダブルクリックで名前を変え、プロパティのショートカット（A、P、S、
  R、T、M、F、E、L、U と2回押しの AA、PP、SS、RR、TT、MM、FF、EE、LL、UU）、Shift でプロパティを
  追加、Alt+Shift+A/P/S/R/T で現在の時刻にキーを打ち、Ctrl+` で選択レイヤーを開閉できます。
- **パペットツール:** レイヤーから作ったメッシュに、位置・詳細・ベンド・スターチ・重なりのピンを
  置けます。リアルタイムのピン記録、回転・拡大縮小のハンドル、リギング用にヌルで動かす（または
  ヌルを運ぶ）ピン、髪や布のフォロースルー。

<p align="center">
  <img src="docs/images/effectcraft-graph-editor.png" alt="テキストアニメーターの範囲開始のイーズ付き値カーブを表示するグラフエディター。ホールド・リニア・自動ベジェ・イージーイーズのボタンバー付き" width="100%">
</p>

## エフェクト

カテゴリにわたる308個のビルトインエフェクト。それぞれにパラメーター名、順序、初期値があります:
ぼかしとシャープ、チャンネル、色調補正（トーンカーブ、レベル補正、色相/彩度、本格的なカラーグレーディングの
パネル…）、変形（ワープ、バルジ、タービュレントディスプレイス、CC Power Pin…）、描画（フラクタルノイズ、
グラデーション、ストローク、ブラシアニメーション、オーディオスペクトラム…）、キーイング、マット、ノイズ＆グレイン、
遠近、**シミュレーション**（CC Particle World、CC Rainfall、シャター、カードダンス、コースティクス、
ウェーブワールド…）、スタイライズ（グロー、CC Glass…）、**時間**（エコー、ポスタリゼーション時間、タイムワープ、
時間置き換え…）、**オーディオ**（リバーブ、パラメトリックEQ、ディレイ、ステレオミキサー…）、
テキスト、トランジション、ユーティリティ、エクスプレッション制御。全9種の**レイヤースタイル**
（ドロップシャドウ、内側／外側グロー、ベベルとエンボス、サテン、オーバーレイ、ストローク）がグローバルライト付きで使えます。
プレビューはオーディオを同期再生し、メーターと波形を表示します。

<p align="center">
  <img src="docs/images/effectcraft-effects.png" alt="グロー付きの CC Particle World のバーストと、パーティクルシステムのパラメーターを並べたエフェクトコントロールパネル" width="100%">
</p>

## 3D

クラシック3Dコンポジット: 向きとマテリアルのオプションを持つ3Dレイヤー、被写界深度付きの
1ノード／2ノードの**カメラ**、ソフトなレイトレース影を持つ**ライト**（パラレル、スポット、ポイント、アンビエント）、
正しく交差するレイヤー、カメラの周回・パン・ドリーのツール、正面・上面・左側・カスタムのビュー。

<p align="center">
  <img src="docs/images/effectcraft-3d.png" alt="カスタムビュー1の3Dショーケースデモ: グリッド床に柔らかい影を落とすスポットライトに照らされた交差するカード。カメラの視錐台とライトがワイヤーフレームで描かれている" width="100%">
</p>

## 書き出し

専用のレンダーキューに、レンダリング設定と出力モジュールがあります: 音声付きの **H.264** の MP4 と
**ProRes** MOV（Proxy から 4444 XQ まで、アルファ付き）、**HEVC**（Main / Main 10）と **AV1** の MP4、
**WebM**（中間フレーム（インターフレーム）とアルファ付きの VP9、または AV1。Opus 音声）、PNG、JPEG、TIFF、Targa、
32ビット EXR の連番、アニメーションGIF、WAV/AIFF。3:2 プルダウンによるフィールドレンダリング、
エフェクト／ソロ／ガイド／深度の上書き、クロップ、関心領域とリサイズ、初期値付きのレンダリング設定と
出力モジュールのテンプレート、レンダリング後の処理、ストレージのオーバーフロー、レンダリングログ。
同じキューはコマンドラインからも動きます。エンコーダーはすべて純Rust（FilmCraft の H.264、ProRes、
AAC と、EffectCraft 自身の VP9、AV1、HEVC、Opus）で、内部に FFmpeg は含まれておらず、依存してもいません。

**Lottie** は双方向に対応しています: ファイル ▸ 書き出し ▸ Lottie JSON… はコンポジション（プリコンポーズ、
シェイプ・ソリッド・画像・テキスト・ヌルのレイヤー、イーズ付きキーフレームと空間キーフレーム、マスク、トラックマット、
描画モード、時間リマップ、任意でエクスプレッション）を `.json` または `.lottie` として書き出し、
Lottie で表現できないものは一覧にします。ファイル ▸ 読み込み ▸ Lottie… はそれを新しいコンポジション
として開きます。

<p align="center">
  <img src="docs/images/effectcraft-render-queue.png" alt="2つのコンポジションが並んだレンダーキューパネル" width="100%">
</p>

## エージェント向け

エンジンのコマンドは CLI、JSON 制御チャンネル、MCP から使えます。UI 専用のコマンドには、起動中の
アプリのフロントエンド・ディスパッチャが必要です。メニューへ完全にアクセスするには、アプリの制御
チャンネルか接続した MCP ブリッジが必要です。ヘッドレスの MCP セッションはサポートされているエンジンコマンドを
公開しますが、UI 専用のコマンドやブリッジ専用の `ui_*` ツールは公開しません。

- **MCP サーバー:** `effectcraft-cli mcp` は stdio 経由で Model Context Protocol で通信し、ヘッドレスでも、
  起動中のアプリにブリッジしても（`--bridge 9877`）動きます。ツールはコマンド、プロジェクトとプロパティ
  ツリー、キーフレーム、レンダリング済みのフレームを扱います。このリポジトリには使える
  [`.mcp.json`](.mcp.json) が入っています。
- **コマンドライン:** JSON を返す単発の呼び出し。たとえば
  `effectcraft-cli set Main '#1' transform/position '[100,360]' --time 0 main.ecproj --save`、
  `effectcraft-cli render --comp Main --out main.mp4`、
  `effectcraft-cli exec file.exportLottie '{"comp":"Main","path":"main.json"}' main.ecproj`。
- **制御チャンネル:** `effectcraft --control 9877` は JSON 行を受け取ってコマンドを実行し、自動化IDで
  任意のウィジェットを調べてクリックし、スクリーンショットを撮ります。

[docs/agents.md](docs/agents.md) と [docs/control-protocol.md](docs/control-protocol.md) を参照してください。

## はじめかた

各バージョンのインストーラーは [Releases](https://github.com/storytold/effectcraft/releases) ページに
あります: ユニバーサルな macOS アプリ、x64・x86・ARM64 向けの Windows MSI とポータブル zip、
x86_64 と aarch64 向けの Linux の AppImage・Flatpak・deb・rpm・tar.gz と riscv64 の tar.gz、
FreeBSD x86_64 の tarball、そして Web ビルド。[ダウンロード](#ダウンロード) のページにすべてのファイルが
一覧されています。Windows ARM64 ビルドは、x64 エミュレーションなしで Windows on ARM 上でネイティブに
動きます。CI は Windows 11 ARM64 実機に MSI をインストールし、両方のプログラムが ARM64 であることを
確かめてコマンドラインツールを動かしますが、ARM64 上でアプリのウィンドウを開いたりテスト一式を
ネイティブに走らせたりはまだしていません。違う動きを見つけたら知らせてください。

Gentoo では、コミュニティの [::snakebyte オーバーレイ](https://github.com/switch87/snakebyte-overlay) が
Linux リリースを `media-video/effectcraft-bin` としてパッケージしています（EffectCraft チームによる
管理ではありません）:

```sh
eselect repository add snakebyte git https://github.com/switch87/snakebyte-overlay.git
emaint sync -r snakebyte
echo 'media-video/effectcraft-bin ~amd64' >> /etc/portage/package.accept_keywords/effectcraft
emerge --ask media-video/effectcraft-bin
```

自分でビルドするには [Rust](https://rustup.rs/) 1.95 以降が必要です。

```sh
git clone https://github.com/storytold/effectcraft
cd effectcraft
cargo run --release -p effectcraft          # the app (add `-- --demo` to open the demo project)
cargo run --release -p effectcraft-cli -- render --out intro.mp4    # render the demo headless
cargo xtask web --serve 8765                # the browser build on http://127.0.0.1:8765/ (docs/web.md)
```

初めてなら、[docs/footage.md](docs/footage.md) がフッテージと画像連番の読み込み、フッテージの解釈を
扱っています。

開発に加わるなら、`cargo test --workspace` を走らせ、コミットのたびに `cargo xtask ci`（整形、lint、
テスト、レイヤリング、アセットのクレジット表記、WebAssembly ビルド）を実行してください。[CONTRIBUTING.md](CONTRIBUTING.md)、
[docs/architecture.md](docs/architecture.md)、[docs/testing.md](docs/testing.md) を参照してください。

## 現状

EffectCraft は若いプロジェクトです。最初のコミットは 2026年10月1日でした。308個のエフェクトを含む
幅広い機能がありますが、機能の幅広さと、あらゆるエッジケースへの完全な対応はイコールではありません。
今は、いろいろ試して、壊れたところを教えてもらうのに向いています。クライアント向けの本番制作で使うことはまだ
おすすめしません。とくに:

- **ネイティブなプロジェクト形式。** プロジェクトは `.ecproj` として保存され、読めるバージョン付き JSON です。
  他社の独自形式のプロジェクトファイルや After Effects SDK のプラグインには対応していません。
- **描画の忠実性はまだ改善の途中です。** 自動のレンダリングテストは今のところ小さなコンポジションが
  中心なので、見た目の食い違いや予期しない動作は知らせてください。
- **macOS がもっともテストされているプラットフォームです。** Linux と Windows の利用者からは
  基本的な操作の不具合が報告されており、それらを優先的に修正しています。
- **機械学習モデルは新たに導入されたものです。** ロトブラシは MobileSAM（設定 ▸ ロトブラシにあるオプションの40MB
  ダウンロード）を、フェイストラッキングは Google の MediaPipe Face Landmarker（設定 ▸ フェイストラッキングにある
  任意の3.8MBのダウンロード）を使えます。どちらも評価と調整の途上です。

EffectCraft は **アルファ版**です。[docs/target-app-parity.md](docs/target-app-parity.md) で追跡している
機能のおよそ91%が存在し、プロの制作に使える状態までの道のりは45%ほどと見ています。
[ROADMAP](ROADMAP.md) に計画、[docs/target-app-parity.md](docs/target-app-parity.md) に測定、
[docs/gaps.md](docs/gaps.md) に既知の不足がすべてあります。Web ビルド（[docs/web.md](docs/web.md)）は
ブラウザでアプリ全体を動かします。[バグ報告](https://github.com/storytold/effectcraft/issues)が、
今いちばん助かるものです。

## 作りかた

- **エンジンファースト。** 厳格なレイヤリングを持つ、小さなクレートの積み重ね。インターフェース（egui）は
  その上にあり、下にあるものはどれもそれに依存しません。
- **クリーンルーム開発。** 公開されたドキュメントと観察できる振る舞いから作り、他社の独自形式のファイル、アイコン、
  プリセット、コードは決して読みません。GPL のプロジェクトからコードを写すこともありません。
  詳しい規則は [AGENTS.md](AGENTS.md) にあります。
- **プライバシー優先がデフォルトです。** テレメトリーはなく、自分で頼まない限りネットワークにも出ません。
- **ひとつのファミリー。** EffectCraft は時間モデルとテキストエンジンの設計を FilmCraft と共有し、
  映像と音声のコーデックもそこから得ています。

## ダウンロード

**EffectCraft が初めてなら？** [getartcraft.com の EffectCraft ページ](https://getartcraft.com/apps/effectcraft)からダウンロードしてください。いちばん簡単な入れかたです。

**特定のビルドや形式が欲しいなら？** GitHub の [最新リリース](https://github.com/storytold/effectcraft/releases/latest)には下のすべてのビルドが、[すべてのリリース](https://github.com/storytold/effectcraft/releases)には以前のバージョンとその説明があります。ファイル名の `<ver>` はバージョン番号で、`SHA256SUMS.txt` にすべてのファイルのチェックサムがあります。

### Windows

| ビルド | インストーラー | ポータブル |
|---|---|---|
| x64（64ビット Intel/AMD） | `effectcraft-<ver>-windows-x64.msi` | `effectcraft-<ver>-windows-x64-portable.zip` |
| arm64（Snapdragon などの ARM PC） | `effectcraft-<ver>-windows-arm64.msi` | `effectcraft-<ver>-windows-arm64-portable.zip` |
| x86（32ビット） | `effectcraft-<ver>-windows-x86.msi` | `effectcraft-<ver>-windows-x86-portable.zip` |

インストーラーと実行ファイルはコード署名されています。インストーラーは、誰のために、どこに
インストールするかを尋ねます。既定では自分のアカウントのためだけに、管理者の確認なしで
`%LOCALAPPDATA%\Programs\ArtCraft\EffectCraft` に入り、「Everyone on this computer」なら
`C:\Program Files\ArtCraft\EffectCraft`（32ビット版は `Program Files (x86)`）に入ります。
以降のバージョンは同じフォルダに入ります。選んだフォルダへサイレントにインストールするには:
`msiexec /i effectcraft-<ver>-windows-x64.msi /qn INSTALLFOLDER="D:\Apps\EffectCraft"`。
サイレントインストールは既定でユーザー単位です。`INSTALLFOLDER` が選ぶのはフォルダであり、
誰のために入れるかではありません。全ユーザー向けに入れるには、管理者権限のターミナルか展開ツールから
`ALLUSERS=1` を付けてください。

### macOS

| ビルド | ファイル | 備考 |
|---|---|---|
| アプリ、ユニバーサル（Apple silicon + Intel） | `effectcraft-<ver>-macos-universal.dmg` | 署名と公証済み |
| コマンドラインツール、ユニバーサル | `effectcraft-cli-<ver>-macos-universal.zip` | 署名と公証済み |

### Linux

| 形式 | x86_64 | aarch64（ARM64） | 備考 |
|---|---|---|---|
| AppImage | `effectcraft-<ver>-linux-x86_64.AppImage` | `effectcraft-<ver>-linux-aarch64.AppImage` | どこでも動きます。[AppImageUpdate](https://github.com/AppImageCommunity/AppImageUpdate)（`.zsync` ファイル）で自分自身を更新します |
| Flatpak | `effectcraft-<ver>-linux-x86_64.flatpak` | `effectcraft-<ver>-linux-aarch64.flatpak` | サンドボックス化されています。`flatpak install --user <file>` |
| Debian/Ubuntu | `effectcraft-<ver>-linux-x86_64.deb` | `effectcraft-<ver>-linux-aarch64.deb` | |
| Fedora/RHEL/openSUSE | `effectcraft-<ver>-linux-x86_64.rpm` | `effectcraft-<ver>-linux-aarch64.rpm` | |
| Tarball | `effectcraft-<ver>-linux-x86_64.tar.gz` | `effectcraft-<ver>-linux-aarch64.tar.gz` | どこにでも展開できます |

RISC-V 64ビット: `effectcraft-<ver>-linux-riscv64.tar.gz`（tarball のみ。glibc 2.39 以降が必要で、
たとえば Ubuntu 24.04 以降や Debian 13 以降）。

**Linux でのファイルのドラッグ＆ドロップ:** 現在の eframe/winit 0.30 のネイティブ Wayland バックエンドは、
ファイルのホバーやドロップのイベントを届けません。Hyprland などのネイティブ Wayland セッションでは、
**ファイル ▸ 読み込み ▸ ファイル…**（複数ファイルを受け付けます）を使ってください。XWayland が入っていて
`DISPLAY` が設定されているなら、XWayland 経由で EffectCraft を起動してください:

```sh
env -u WAYLAND_DISPLAY effectcraft
```

バックエンドがイベントを届ける場合（X11/XWayland、Windows、macOS）、ウィンドウのどこにでもメディアを
落とせば読み込まれ、ホバー中はドロップ先のヒントが出ます。ネイティブ Wayland では、イベントを
発生しない操作はアプリからは検出できないため、代わりにファイル ▸ 読み込みの代替を案内する起動時の通知を
出します。

### FreeBSD

| ビルド | ファイル |
|---|---|
| x86_64 | `effectcraft-<ver>-freebsd-x86_64.tar.gz` |

### Web（WebAssembly）

| ビルド | ファイル | 備考 |
|---|---|---|
| 静的サイト | `effectcraft-web-<ver>.zip` | モダンなブラウザで動きます。任意の静的サーバーで配信してください |

## Crafting Apps

EffectCraft は **Crafting Apps** のひとつです。[ArtCraft](https://getartcraft.com/)
チームによる無料のオープンソースの制作ツールで、どれも Rust で一から書かれ、単体で成り立ちます。

| | アプリ | 用途 | コード | 詳細 |
|:-:|---|---|---|---|
| <img src="https://raw.githubusercontent.com/storytold/photocraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.photocraft.png" alt="" width="32" height="32"> | **PhotoCraft** | 画像編集: レイヤー、マスク、文字、本物の PSD ファイル | [GitHub](https://github.com/storytold/photocraft) | [ウェブサイト](https://getartcraft.com/apps/photocraft) |
| <img src="https://raw.githubusercontent.com/storytold/vectorcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.vectorcraft.png" alt="" width="32" height="32"> | **VectorCraft** | ベクターイラスト | [GitHub](https://github.com/storytold/vectorcraft) | [ウェブサイト](https://getartcraft.com/apps/vectorcraft) |
| <img src="https://raw.githubusercontent.com/storytold/filmcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.filmcraft.png" alt="" width="32" height="32"> | **FilmCraft** | 動画編集、カラー、音 | [GitHub](https://github.com/storytold/filmcraft) | [ウェブサイト](https://getartcraft.com/apps/filmcraft) |
| <img src="https://raw.githubusercontent.com/storytold/lightcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.lightcraft.png" alt="" width="32" height="32"> | **LightCraft** | 写真ライブラリと RAW 現像 | [GitHub](https://github.com/storytold/lightcraft) | [ウェブサイト](https://getartcraft.com/apps/lightcraft) |
| <img src="https://raw.githubusercontent.com/storytold/pdfcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.pdfcraft.png" alt="" width="32" height="32"> | **PdfCraft** | PDF の閲覧、整理、保護 | [GitHub](https://github.com/storytold/pdfcraft) | [ウェブサイト](https://getartcraft.com/apps/pdfcraft) |
| <img src="https://raw.githubusercontent.com/storytold/effectcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.effectcraft.png" alt="" width="32" height="32"> | **EffectCraft** | モーショングラフィックスと視覚効果 | [GitHub](https://github.com/storytold/effectcraft) | [ウェブサイト](https://getartcraft.com/apps/effectcraft) |
| <img src="https://raw.githubusercontent.com/storytold/designcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.designcraft.png" alt="" width="32" height="32"> | **DesignCraft** | ページレイアウトと出版 | [GitHub](https://github.com/storytold/designcraft) | [ウェブサイト](https://getartcraft.com/apps/designcraft) |
| <img src="https://raw.githubusercontent.com/storytold/figcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.figcraft.png" alt="" width="32" height="32"> | **FigCraft** | インターフェースデザインとプロトタイピング | [GitHub](https://github.com/storytold/figcraft) | [ウェブサイト](https://getartcraft.com/apps/figcraft) |
| <img src="https://raw.githubusercontent.com/storytold/wordcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.wordcraft.png" alt="" width="32" height="32"> | **WordCraft** | ワープロと文書 | [GitHub](https://github.com/storytold/wordcraft) | [ウェブサイト](https://getartcraft.com/apps/wordcraft) |
| <img src="https://raw.githubusercontent.com/storytold/soundcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.soundcraft.png" alt="" width="32" height="32"> | **SoundCraft** | オーディオの録音と編集 | [GitHub](https://github.com/storytold/soundcraft) | [ウェブサイト](https://getartcraft.com/apps/soundcraft) |
| <img src="https://raw.githubusercontent.com/storytold/deckcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.deckcraft.png" alt="" width="32" height="32"> | **DeckCraft** | プレゼンテーションとスライド | [GitHub](https://github.com/storytold/deckcraft) | [ウェブサイト](https://getartcraft.com/apps/deckcraft) |
| <img src="https://raw.githubusercontent.com/storytold/cadcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.cadcraft.png" alt="" width="32" height="32"> | **CADCraft** | コンピューター支援設計 | [GitHub](https://github.com/storytold/cadcraft) | [ウェブサイト](https://getartcraft.com/apps/cadcraft) |
| <img src="https://raw.githubusercontent.com/storytold/gridcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.gridcraft.png" alt="" width="32" height="32"> | **GridCraft** | 表計算とデータ | [GitHub](https://github.com/storytold/gridcraft) | [ウェブサイト](https://getartcraft.com/apps/gridcraft) |
| <img src="https://raw.githubusercontent.com/storytold/texturecraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.texturecraft.png" alt="" width="32" height="32"> | **TextureCraft** | 3D テクスチャペイント | [GitHub](https://github.com/storytold/texturecraft) | [ウェブサイト](https://getartcraft.com/apps/texturecraft) |
| <img src="https://raw.githubusercontent.com/storytold/craft-anything/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.craft-anything.png" alt="" width="32" height="32"> | **Craft Anything** | あらゆるソフトの仕組みを見て、自分で作れるように | [GitHub](https://github.com/storytold/craft-anything) | [ウェブサイト](https://getartcraft.com/apps/craft-anything) |
| <img src="https://raw.githubusercontent.com/tangency-labs/solidcraft/main/assets/icons/app/linux/exports/hicolor/64x64/apps/io.tangencylabs.solidcraft.png" alt="" width="32" height="32"> | **SolidCraft** | Tangency Labs によるパラメトリック3D CAD（コミュニティアプリ） | [GitHub](https://github.com/tangency-labs/solidcraft) | |

そして [**ArtCraft**](https://getartcraft.com/) そのもの。本当のコントロールを求めるアーティストのための、
私たちの AI 画像・映像スタジオです。

<br>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="Join the ArtCraft community on Discord" src="https://img.shields.io/badge/Join%20us%20on%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<h3 align="center">一緒に何か作りましょう</h3>

<p align="center">
  私たちの Discord は、あらゆるアーティストが集まる場所です。絵を描く人、撮る人、線を引く人、
  フィルムを切る人、文字を組む人、そして自分が何を作るのが好きなのか、まだ探している人。
  いま取り組んでいるものを見せてください。助けを求めてください。壊れているものを教えてください。
  このツールでこうできたらいいのに、という希望を聞かせてください。
  どんな媒体でも、始めてどのくらい経っていても、ここはあなたの居場所です。
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><b>discord.gg/artcraft</b></a> ·
  <a href="https://getartcraft.com/">getartcraft.com</a> ·
  <a href="https://getartcraft.com/apps">Crafting Apps</a> ·
  <a href="https://getartcraft.com/apps/effectcraft">EffectCraft</a>
</p>

## ライセンスとクレジット

EffectCraft は [MIT](LICENSE-MIT) または [Apache-2.0](LICENSE-APACHE) のデュアルライセンスで、どちらかを選べます。
Copyright (c) 2026 ArtCraft Team and the EffectCraft contributors. 必要な表示は [NOTICE](NOTICE) にあります。

同梱のフォント、アイコン、画像、その他のアセットは、それぞれのオープンライセンスを保ちます。
ひとつひとつが作者、出所、ライセンスとともに [ATTRIBUTION.md](ATTRIBUTION.md) に並んでいます。

[`docs/brand/`](docs/brand/) にある ArtCraft の名称、ワードマーク、ロゴは ArtCraft チームの商標で、
このライセンスの対象ではありません。変更せずに、このリポジトリと EffectCraft の一部としてのみ、
[`docs/brand/LICENSE-brand.txt`](docs/brand/LICENSE-brand.txt) のもとで使えます。
フォークや改変版はこれらを取り除かなければなりません。

<p align="center">
  <a href="https://getartcraft.com/"><img alt="ArtCraft" src="docs/brand/artcraft-mark.svg" width="28"></a><br>
  <sub><a href="https://getartcraft.com/">ArtCraft</a> チームとコミュニティが作りました。</sub>
</p>

## スター履歴

[![スター履歴のグラフ](https://api.star-history.com/svg?repos=storytold/effectcraft&type=Date&legend=top-left)](https://www.star-history.com/?repos=storytold%2Feffectcraft&type=date&legend=top-left)
