# 設定ライブラリーからのメッセージ: なぜ書き込みを断ったのか
# （native-term-config を参照）。引用される名前・パス・ssh 自身の
# 出力は訳さない。

## 入力されたホストやフォルダー
config-hostname-blank = アドレス { $value } が空か、空白を含んでいます
config-name-one-line = 名前は空でない 1 行にしてください
config-folder-name-one-line = フォルダー名は空でない 1 行にしてください
config-value-blank = { $what } { $value } が空か、空白を含んでいます
config-note-one-line = メモは 1 行にしてください
config-login-one-line = ログイン後に実行するコマンドは 1 行にしてください
config-credential-set = 認証セット { $value }: 空白・引用符・/ \ : * ? は使えません
config-persistent-host = 常駐セッション { $value }: tmux、tmux-log、screen、off のいずれかです
config-persistent-folder = 常駐セッション { $value }: tmux、tmux-log、screen のいずれかです
config-tab-color-host = タブの色 { $value }: 色名、#RRGGBB、または none です
config-tab-color-folder = タブの色 { $value }: 色名または #RRGGBB です
config-color-scheme-host = 配色 { $value }: 配色名または none です
config-color-scheme-folder = 配色 { $value }
config-port-zero = ポートに 0 は使えません
config-name-taken = { $name } はすでに使われています

## この操作がどの種類のセッション向けか
config-not-ssh = { $alias } は ssh のホストではありません
config-is-ssh = { $alias } は ssh のホストです
config-folder-options-main = フォルダーのオプションはフォルダーファイル用で、メインの設定には書けません
config-main-no-folder-options = メインの設定にフォルダーの設定はありません

## セッションを置くフォルダーの移動
config-not-full-path = 完全なパスではありません
config-folders-here = そこが今フォルダーのある場所です
config-folders-already = { $path } にはすでにセッションのフォルダーがあります
config-folders-none = { $path } にセッションのフォルダーがありません
config-file-unreadable = { $path }: { $error }

## 読んだときにはあったのに、今は無いもの
config-missing = { $what } が見つかりません（NativeTerm の外で変更されましたか？）
config-missing-host = ホスト { $alias }
config-missing-host-in = { $path } のホスト { $alias }
config-missing-session = セッション { $name }

## 書き込み
config-write-conflict = このファイルは読み込んだあとに別の場所から変更されています
config-write-rejected = ssh がこの変更を受け付けなかったため、前の版に戻しました: { $reason }
config-include-wildcard = { $pattern }: ディレクトリ名のワイルドカードは展開しません
