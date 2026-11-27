# Regent Launcher 产品规格

Regent Launcher 是 Paradox 游戏的启动器，用 Rust 和 Tauri（系统 WebView）写，不用 Electron。

## MVP：先能用来开一局

### 游戏识别

- 自动找到 Steam、GOG、Xbox 三个渠道的安装路径。三个平台的路径规则各写一套。
- 从游戏目录里的 `launcher-settings.json` 读出真正的 exe 路径和默认启动参数。
- 第一版只支持 Stellaris、HOI4、EU4。这三款的 mod 生态最大，加载机制也最统一。

### Mod 列表

- 扫描 `Documents/Paradox Interactive/<游戏>/mod/*.mod` 和 Steam 创意工坊目录，解析 descriptor：名称、版本、`supported_version`、依赖。
- 列表做虚拟滚动，一千个 mod 也不能卡。这是和 Electron 版拉开差距的第一个瞬间。
- 支持启用、禁用、拖拽排序、搜索。
- 游戏版本不匹配的 mod 标黄。

### Playset（mod 组合）

- 新建、复制、重命名、切换。
- 存在自己的数据库里，启动时再写成游戏认的格式，多数游戏是 `dlc_load.json`。

### DLC 开关

- 只列出已安装的 DLC，只做启用和禁用。
- 这条线要守住：不做解锁器，否则项目会出事。

### 一键启动

- 跳过官方启动器，直接拉起游戏 exe。
- 可以自定义启动参数，比如 debug 模式。

## v1：别人没做好的部分，也就是卖点

### 不进游戏改设置

- 解析 `settings.txt` 和 `pdx_settings.txt`（Clausewitz 格式），做成表单：分辨率、全屏、垂直同步、UI 缩放、语言、自动存档频率、音量。
- 认不出的字段放进"高级"里，用原始编辑器兜底。
- 每次写入前自动备份，改坏了能一键回滚。

### Mod 冲突检测

- 两个 mod 覆盖同一个文件路径时，标出谁覆盖了谁，并给出建议顺序。
- Irony Mod Manager 已经做了这个，所以要做得更快、更好看。

### 成就和 checksum 提示

- 实时显示当前组合是否兼容 Ironman 成就，哪些 mod 改了 checksum，哪些没改。
- 这是玩家真正关心、官方却从来不直接告诉你的东西。

### 存档和 Playset 绑定

- 列出存档，并记住每个存档是用哪个 playset 开的。点存档就自动切回对应的 mod 组合再启动。
- 这是杀手级功能。P 社玩家最常见的惨案就是换了 mod 再读档，存档直接坏掉。

### 崩溃诊断

- 游戏崩溃后自动读取 `error.log`，把报错归到具体的 mod 上，告诉你"大概率是这个 mod 炸的"。

### 分享 Playset

- 导出成一串分享码或一个 JSON。联机时朋友一键导入，并核对双方的 mod 列表和 checksum 是否一致。
- 兼容导入官方启动器和 Irony 的格式，降低别人迁移过来的成本。
