# 配置库的提示：为什么某次修改没有写下去（见 native-term-config）。
# 其中引用的名称、路径和 ssh 自己的输出不翻译。

## 输入的主机或文件夹
config-hostname-blank = 地址 { $value } 是空的或含空格
config-name-one-line = 名称必须是一行且不能为空
config-folder-name-one-line = 文件夹名必须是一行且不能为空
config-value-blank = { $what } { $value } 是空的或含空格
config-note-one-line = 备注只能写一行
config-login-one-line = 登录后执行的命令只能写一行
config-credential-set = 凭据集 { $value }：不能有空格、引号或 / \ : * ?
config-persistent-host = 常驻会话 { $value }：只能是 tmux、tmux-log、screen 或 off
config-persistent-folder = 常驻会话 { $value }：只能是 tmux、tmux-log 或 screen
config-tab-color-host = 标签颜色 { $value }：颜色名、#RRGGBB 或 none
config-tab-color-folder = 标签颜色 { $value }：颜色名或 #RRGGBB
config-color-scheme-host = 配色方案 { $value }：方案名或 none
config-color-scheme-folder = 配色方案 { $value }
config-port-zero = 端口不能是 0
config-name-taken = { $name } 已被占用

## 这个操作对应的是哪一类会话
config-not-ssh = { $alias } 不是 ssh 主机
config-is-ssh = { $alias } 是 ssh 主机
config-folder-options-main = 文件夹选项属于文件夹文件，不属于主配置
config-main-no-folder-options = 主配置里没有文件夹设置

## 移动存放会话的文件夹
config-not-full-path = 不是完整路径
config-folders-here = 这里就是文件夹当前所在的位置
config-folders-already = { $path } 里已经有会话文件夹了
config-folders-none = { $path } 里没有会话文件夹
config-file-unreadable = { $path }：{ $error }

## 读的时候还在，现在没了
config-missing = 找不到{ $what }（在 NativeTerm 之外改动过？）
config-missing-host = 主机 { $alias }
config-missing-host-in = { $path } 里的主机 { $alias }
config-missing-session = 会话 { $name }

## 写入
config-write-conflict = 这个文件在读取之后被别处改动过
config-write-rejected = ssh 不接受这次修改，已还原成上一个版本：{ $reason }
config-include-wildcard = { $pattern }：目录名里的通配符不会展开
