## 顶栏和设置

settings-toggle = 设置
import-securecrt-button = 从 SecureCRT 导入…
language-label = 语言
language-system = 跟随系统
theme-label = 外观
theme-system = 跟随系统
theme-light = 浅色
theme-dark = 深色
auto-reconnect-setting = 断线后自动重连（登录失败不会重试）
dock-pin = 钉住
dock-pin-hint = 已停靠在屏幕{ $edge ->
        [top] 顶部
        [left] 左侧
       *[right] 右侧
    }：鼠标离开后自动收起，钉住后不收起。把窗口拖离边缘即可取消停靠。
settings-terminal = Windows Terminal：{ $dir }（{ $kind }）
terminal-kind-packaged = 商店版
terminal-kind-portable = 便携版
terminal-kind-unpackaged = 解压版
settings-profile = “NativeTerm SSH”配置：{ $status }

## 会话树

tree-heading = 会话
tree-reload-hint = 重新读取 ~/.ssh
tree-new-folder = 文件夹
tree-search-hint = 搜索名称、主机、用户、备注…（Ctrl+F）
tree-search-clear = 清除搜索（Esc）
tree-no-match = 没有匹配“{ $query }”的主机
tree-recent = 最近使用
tree-all = 全部会话
tree-empty = 还没有主机：右键点文件夹新建主机，或先新建文件夹。
tree-main-config = ~/.ssh/config
fab-open = NativeTerm：连接、标签、会话
fab-close = 关闭（Esc）
fab-search-hint = 主机名，或 user@host[:port]
fab-active = 当前会话：{ $label }
fab-no-active = 当前 Terminal 标签里没有 NativeTerm 会话。
fab-show-main = 显示 NativeTerm
quick-connect = 连接到 { $target }
quick-save = 保存…
quick-save-hint = 保存为 ~/.ssh/config 中的主机
menu-connect-all = 全部连接
menu-connect-all-new-window = 在新窗口中全部连接
menu-new-host = 新建主机…
menu-rename-folder = 重命名文件夹…
menu-connect = 连接
menu-connect-new-window = 在新窗口中连接
menu-edit = 编辑…
menu-move-to = 移动到
menu-delete = 删除…
host-via = 经由 { $jump }
host-alias = 别名 { $alias }

## 打开的会话

sessions-heading = 打开的会话（{ $count }）
view-tabs = 所有标签
view-tabs-hint = 所有 Windows Terminal 窗口里的全部标签，包括你自己的（Ctrl+T）
tabs-search-hint = 搜索标签标题…（Ctrl+T）
tabs-none = 没有找到 Windows Terminal 标签。
tabs-no-match = 没有匹配的标签。
tabs-window = 窗口 { $number } · { $count } 个标签
sessions-clear-finished = 清除已结束的
sessions-restored-waiting = 有 { $count } 个恢复的会话等待连接。
sessions-connect-all = 全部连接
sessions-close-all = 全部关闭
sessions-one-by-one = 也可以在下面逐个连接。
sessions-empty = 双击主机即可连接；右键点主机或文件夹有更多操作。
session-attempt = 第 { $n } 次连接
session-auto-reconnect = 自动重连 { $n }
session-location = 窗口 { $window } · 标签 { $tab }
session-selected = 当前标签
session-split = 已拆分
session-current-title = 当前标题：{ $title }
session-not-located = 未定位
button-focus = 切换到
button-connect = 连接
button-reconnect = 重连
button-disconnect = 断开
button-close = 关闭
button-save = 保存
button-cancel = 取消
button-delete = 删除
button-check-again = 重新检查

## 会话状态

state-opening = 正在打开
state-detached = 正在寻找标签…
state-waiting = 已恢复，未连接
state-connecting = 正在连接 / 等待登录
state-connected = 已连接
state-login-failed = 登录失败（{ $code }）
state-disconnected = 已断开（{ $code }）
state-ended = 已结束（{ $code }）
state-failed = 失败：{ $reason }
state-gone = 标签已不在
state-closed = 已关闭

## 提示

notice-lost-sessions = 上次运行的 { $count } 个会话已找不到标签
notice-tab-not-found = { $label }：没有找到它的标签（拆分的标签被选中后才能找到）
notice-tab-gone = 标签“{ $title }”已不在
notice-tab-changed = { $label }：标签已变化，未关闭
notice-not-linked = { $label }：它的标签没有连接到 NativeTerm
notice-terminal-failed = 无法启动 Windows Terminal：{ $error }
notice-tabs-pending = { $count } 个标签没有打开：另一个 Terminal 窗口成了活动窗口
notice-tabs-missing = { $count } 个标签没有出现在 Windows Terminal 中：{ $labels }
notice-pipe-stopped = NativeTerm 的管道服务已停止：{ $error }
notice-old-shim = 连接进来的 shim 协议版本是 { $protocol }（应为 { $expected }），请更新
notice-gave-up = { $label }：已重连 { $tries } 次，不再重试
notice-db-unavailable = { $path }：{ $error }；打开的会话不会被记住
notice-no-data-dir = 没有数据目录：{ $error }；打开的会话不会被记住
notice-shim-missing = 找不到 { $path }，无法打开标签
notice-no-pipe = NativeTerm 无法提供管道服务：{ $error }
notice-tab-menu-unavailable = NativeTerm 的标签菜单不可用：{ $error }
notice-move-failed = 移动 { $alias } 失败：{ $error }
error-host-gone = { $alias } 已不存在（在 NativeTerm 之外被修改了？）
fatal-title = NativeTerm 无法启动
fatal-window = NativeTerm 的窗口无法启动：{ $error }

## “NativeTerm SSH”配置

profile-updated = 已更新“NativeTerm SSH”配置：{ $old } 已移到 { $new }
profile-update-failed = 无法更新“NativeTerm SSH”配置：{ $error }
profile-installed = 已安装（片段）
profile-in-settings = 在这个 Terminal 的 settings.json 里定义
profile-outdated = 指向 { $path }
profile-disabled = 已在 Terminal 的“扩展”页面关闭
profile-missing = 未安装
profile-banner-disabled = “NativeTerm SSH”配置在 Windows Terminal 中已关闭（设置 → 扩展 → NativeTerm）。
profile-banner-missing = Windows Terminal 还没有“NativeTerm SSH”配置，无法打开标签。
profile-install = 安装配置
profile-install-hint = 写入 { $path }（这个用户的所有 Windows Terminal 都会读取）
profile-install-update = 安装 / 更新
profile-remove = 移除
profile-install-failed = 安装配置失败：{ $error }
profile-remove-failed = 移除配置失败：{ $error }
profile-no-localappdata = 没有设置 LOCALAPPDATA

## 主机和文件夹对话框

host-new-title = 在 { $folder } 中新建主机
host-edit-title = 编辑 { $alias }
host-bad-port = 端口“{ $port }”不是 1 到 65535 之间的数字
field-name = 名称
field-name-hint = 显示在会话树和标签上
field-host = 主机
field-host-hint = 主机名或地址
field-user = 用户
field-user-hint = （ssh 默认）
field-port = 端口
field-jump = 跳板机
field-jump-hint = 例如 bastion 或 user@bastion:22
field-keys = 密钥
field-keys-hint = 每行一个 IdentityFile，例如 ~/.ssh/id_ed25519
field-note = 备注
field-note-hint = 一行
host-alias-kept = ssh 别名：{ $alias }（保持不变，标签和脚本照常可用）
folder-new-title = 新建文件夹
folder-rename-title = 重命名 { $name }
folder-name-hint = 文件夹名称
delete-title = 删除主机
delete-question = 从 ssh 配置中删除“{ $label }”（{ $alias }）吗？
delete-backup-note = 文件的备份会保存在数据目录中。

## 从 SecureCRT 导入

import-title = 从 SecureCRT 导入
import-folder-label = SecureCRT 配置文件夹
import-into = 导入到：{ $path }（改动的文件都会先备份）
import-not-found = 没有找到 SecureCRT 的配置文件夹，请在上面填写。
import-preview = 预览
import-run = 导入 { $count } 台主机
import-checking = 每个文件夹写入后都会用 ssh -G 检查。
import-done = 已导入 { $hosts } 台主机，共 { $folders } 个文件夹。
import-folder-failed = 文件夹 { $folder } 没有写入：{ $error }
import-failed = 导入失败：{ $error }
summary-found = 找到 { $sessions } 个会话，分布在 { $folders } 个文件夹；将导入 { $hosts } 台主机，写入 { $new } 个新文件夹和 { $existing } 个已有文件夹
summary-skipped = 已跳过，{ $reason }：{ $count }
skip-already = 之前已导入
skip-plink-later = { $protocol }：暂不支持（计划通过 plink 支持）
skip-protocol = { $protocol }：不是 NativeTerm 打开的终端会话
skip-no-hostname = 没有主机名
summary-duplicates = 多个会话的主机、端口和用户相同：{ $count } 组（都会导入，请检查）
summary-firewall = 防火墙/代理“{ $name }”不会导入：{ $count } 个会话将直接连接
summary-unresolved-jumps = 跳板会话不存在或未导入：{ $count } 个会话将直接连接
summary-logon-actions = 登录动作/脚本不会导入：{ $count } 个会话
summary-saved-passwords = 保存的密码不会导入（{ $count } 个会话）；请使用密钥或 ssh-agent
summary-encodings = 使用非 UTF-8 字符集（OpenSSH 会话使用 UTF-8）：{ $count } 个会话
summary-bad-forwards = 无法读取的端口转发已略过：{ $count } 个会话
summary-also = 一并导入：{ $items }
summary-forwards = { $count } 条端口转发
summary-keys = { $count } 个密钥文件（须为 OpenSSH 格式）
summary-descriptions = { $count } 条多行描述已合并为一行
summary-unreadable = 无法读取的会话文件：{ $count }
summary-not-utf8 = 不是 UTF-8 的会话文件（无法识别的字符已替换）：{ $count }
summary-not-yet = 暂不导入：主机密钥（KnownHosts）和保存的命令

## 标签菜单（Windows Terminal 中 NativeTerm 的标签）

tabmenu-header-mixed = { $label } · 这个标签还有其他窗格
tabmenu-header-titled = { $label } · 标题为“{ $title }”
tabmenu-connect = 连接
tabmenu-reconnect = 重连
tabmenu-disconnect = 断开
tabmenu-clone = 克隆会话
tabmenu-close = 关闭
tabmenu-close-mixed = 关闭此会话（保留其他窗格）
tabmenu-close-others = 关闭其他 NativeTerm 标签
tabmenu-close-disconnected = 关闭已断开的标签
tabmenu-close-right = 关闭右侧标签
