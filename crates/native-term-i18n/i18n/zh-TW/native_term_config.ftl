# 設定庫的提示：為什麼某次修改沒有寫下去（見 native-term-config）。
# 其中引用的名稱、路徑和 ssh 自己的輸出不翻譯。

## 輸入的主機或資料夾
config-hostname-blank = 位址 { $value } 是空的或含空格
config-name-one-line = 名稱必須是一行且不能為空
config-folder-name-one-line = 資料夾名必須是一行且不能為空
config-value-blank = { $what } { $value } 是空的或含空格
config-note-one-line = 備註只能寫一行
config-login-one-line = 登入後執行的命令只能寫一行
config-credential-set = 認證集 { $value }：不能有空格、引號或 / \ : * ?
config-persistent-host = 常駐工作階段 { $value }：只能是 tmux、tmux-log、screen 或 off
config-persistent-folder = 常駐工作階段 { $value }：只能是 tmux、tmux-log 或 screen
config-tab-color-host = 分頁顏色 { $value }：顏色名、#RRGGBB 或 none
config-tab-color-folder = 分頁顏色 { $value }：顏色名或 #RRGGBB
config-color-scheme-host = 配色方案 { $value }：方案名或 none
config-color-scheme-folder = 配色方案 { $value }
config-port-zero = 埠不能是 0
config-name-taken = { $name } 已被佔用

## 這個操作對應的是哪一類工作階段
config-not-ssh = { $alias } 不是 ssh 主機
config-is-ssh = { $alias } 是 ssh 主機
config-folder-options-main = 資料夾選項屬於資料夾檔案，不屬於主設定
config-main-no-folder-options = 主設定裡沒有資料夾設定

## 移動存放工作階段的資料夾
config-not-full-path = 不是完整路徑
config-folders-here = 這裡就是資料夾目前所在的位置
config-folders-already = { $path } 裡已經有工作階段資料夾了
config-folders-none = { $path } 裡沒有工作階段資料夾
config-file-unreadable = { $path }：{ $error }

## 讀的時候還在，現在沒了
config-missing = 找不到{ $what }（在 NativeTerm 之外改動過？）
config-missing-host = 主機 { $alias }
config-missing-host-in = { $path } 裡的主機 { $alias }
config-missing-session = 工作階段 { $name }

## 寫入
config-write-conflict = 這個檔案在讀取之後被別處改動過
config-write-rejected = ssh 不接受這次修改，已還原成上一個版本：{ $reason }
config-include-wildcard = { $pattern }：目錄名裡的萬用字元不會展開
