# ROADMAP — “一定程度直接取代 Photoshop” 路线（v0.5.0 之后 · UX 阶段）

> 交接文档：每项都附实现要点与 `file:line` 锚点，供新会话不重读全库即可
> 开工。更新于 2026-07-24（**v0.12.0 已发布**：全量 debug + 协同性审计
> 批次——77 项对抗验证 findings 全修/有据取舍，蒙版行点击=选择恢复、
> recipe.json 统一持久化契约、未保存保护、指针状态机清残留、引擎位图/
> feather/roundness 语义钉死、web/CLI 协同；详见下方「当前状态」首条。
> **v0.13.0 已发布（2026-07-25）**：边车磁盘布局改为中央库+路径键
> （%LOCALAPPDATA%/autoshop/，按照片绝对路径消歧；./out 只留导出成品
> 图；打开时自动迁移旧 ./out 边车），解决同名照片边车相撞 + cwd 相对
> 性。详见「当前状态」首条。
> 前版 v0.11.2（2026-07-17，AI 分析认证修复：`--bare` 永不读 OAuth →
> 三旗标替代 + env_remove(ANTHROPIC_API_KEY) + 透出 stdout JSON 真实
> 错误，src/advisor/claude.rs）；v0.11.1（2026-07-14，中性 cwd 根治
> 信任门）；v0.11.0 → `e3a4096`（perf #3-B 全清）；v0.10.0 →
> `c312a9f`（边车恢复+XMP 导入）；v0.9.0 → `ca6f73e`（GUI i18n）；
> v0.8.1（preview lag root-fix）；v0.8.0 → `1c1ea36`（zoned fit）。
> 反馈驱动阶段——用户试用 → 报障/提需 → 修复/打磨 → 发布）。

## 当前状态（已完成，勿重做）

- **第三轮 64 路 gpt-5.6-sol 最高推理舰队全量复审 + 修复批（2026-08-03，
  用户令"派发64路并行codex gpt 5.6 sol最高推理等级进行全面代码审查…
  多跑两遍，跑到收敛"；中途按用户令全面切换到订阅 codex 通道）**——
  64/64 单元产出（56 存量 + 8 订阅补跑 + 1 重发），190 findings 三轮
  分批裁决：**~120 确认全修 / 10 驳回有据 / 9 记录立项**，跨 24 文件。
  要点（细节见会话台账 adjudication64.md Batch 1-7）：
  - **数据安全**：CLI Analyze/Auto/Match 三命令补备份门（HIGH，三端契约
    补齐）；迁移 cwd 旁观者文件保护（HIGH）；store 原子拷贝/vMAX 拒绝/
    崩溃残留冻结栅格重置/死引用永久惰性/pixels.json 三竞态；write_recipe
    陈旧 .bak 预清 + 目录拒写；guard_readonly 别名根治 + 导出对渲染源与
    原 RAW 双重把关；分区反推栅格改**每次 fit 唯一认领名**
    （store::claim_raster，GUI+CLI 共用——写在配方落盘前的就地重写窗口
    根除）；fit 计算成果在写库失败时降级为画布未保存而非丢弃；持久化
    fit 同步 clear_pixel_source（配对语义）；设置文件 tmp+rename +
    unix 0600。
  - **校准语义钉死（InPlace=中性显影，Generated=像素含观感）**：Analyze/
    Reset/版本载入/批量导出四路径改只对 Generated 剥 base_curve+
    lens_profile（原 base.is_some() 误伤 InPlace 全部纠正）；Before 四处
    改按画布配方曲线渲染（InPlace 不再假暗 0.6-1.4EV）；Generated 恢复
    时 saved_recipe 同步归一化（假 ● 根除）；两处 v0.14 前"烘焙像素自带
    相机观感"陈旧注释改真相。
  - **GUI 交互**：同路径重开保留未保存工作（HIGH）；Save-all 活画布无条件
    压制陈旧 stash + busy 窗口竞态修；快捷键两级门（Ctrl+O/E/S 焦点下
    可用）；退出层 Enter 默认真可用（布防即交焦点）；WB 滴管逐像素采样；
    HiDPI 1:1/百分比修；radial 边把手增量拖拽；放置四角映射+最小尺寸；
    画笔局部尺度换算；定比角部主导轴按拖拽增量；蒙版拖拽越界守卫
    （panic）；亮度端点 clamp 防振荡；CA 缺通道按通道补层；重试/清扫：
    五个修饰 worker 失败释放 0 字节认领名；busy 期重绘 100ms 节流。
  - **web**：换目录全代际失效+清网格；选照清 debounce 竞态；fill∥heal
    互斥；r.ok 先行真相化 stale 消息；pageOffset 成功才提交；画布重设
    保留已画蒙版；设置窗加载门+代际；saveXmp 防重叠；700px 响应式断点。
  - **解码/元数据**：解码上限有界化（65536²/4GiB，替代 no_limits）；
    烘焙图应用 EXIF 方向（导入 JPEG 不再侧躺）；Meta 尺寸改显示帧
    （竖拍风格特征修正）；APEX/有理数非有限过滤；直方图只降采样。
  - **XMP/eval/style**：feather 文本形状消歧（自家 1% 往返不再变 100%）；
    CropAngle 按 HasCrop 门控；元组严格解析防移位；出处检测改属性形匹配；
    as-shot WB 溯源规则进 eval+style（相机 Kelvin 不再算用户编辑/风格
    偏好）；style 温度混合不再强改 as-shot；索引 tmp pid 唯一；不可读
    sidecar 跳过样本。retouch：full_res 对烘焙源生效；空修复计划拒绝
    写零斑点母版。denoise/segment：原子发布蒙版、.part/tmp 清扫、NaN
    强度防线。图库导入改单次目录扫描（migrate_legacy_from_many，
    O(photos×entries) 根除）。i18n 文案真相三条。
  - **驳回有据（10）**：paste 继承源曲线/范围蒙版参考图无几何/覆盖层
    display-only 降采样/Tab=LR 键盘语法/overlay_ref 全交换点清点/busy
    双门挡陈旧 Fitted/produce_recipe 无 saved 早退/omit 独立成列/web
    Reset 已回填校准/provider 翻转有 same_base 自失效。
  - **记录立项（9）**：load_active/overlay 结算帧同步显影异步化；12× 缩放
    帽 vs 真 1:1；全幅解码转预览内存（并入结构性内存池）；fit 深层 DSP
    四项（饱和度次序非对易/异色净额否决/中性总体身份/岭回归活跃集）；
    分区栅格孤儿清扫。
  基线 **137 lib + 9 gui** 双配置全绿，clippy --all-targets 双零，
  i18n 432 调用点/461 条目 0 未译 0 孤儿。**未发布**（下次发版=minor bump）。

- **v0.15.0 RELEASED（2026-08-03）**——内容 = 自 v0.14.3 后全部四提交：
  在案后续工作全清批（a8f67d2：pixels.json 像素持久化 / worker 取消+进度 /
  61MP 内存八子项 / web 区域几何逆映射 / 字体·模型指纹·临时清扫）、用户
  config dotenv_override（8e9ae1f）、第二轮 32 路舰队 30 项修复 + 死代码
  五角度清查（b2591e5）。发版细节（tag、资产字节）见 git tag v0.15.0 与
  GitHub Release 页。

- **第二轮 32 路 gpt-5.6-sol 舰队复审 + 修复批 & 死代码清查（2026-08-03，
  用户令"再开32路…确保项目实现完美"+"清理一轮死代码（多次确认）"）**——
  fleet32b（契约头补全清批新设计）32/32 成功，115 findings
  （13H/45M/25L/28UX/4info，43.5万 in/8.7万 out tokens），逐条三裁：
  - **HIGH 6 修**：批量导出改从 pixels.json 母版渲染（配方叠其上——批量
    导不再丢修饰）；Ctrl+S 次序重排（像素身份先于徽章/基线/暂存推进，
    pixels 写失败保留 stash）；恢复路径 Generated 剥 base_curve/lens_profile
    （生成像素自带观感，防双重烹调）+ 两处复活的"母版自带相机观感"撒谎
    注释改中性显影真相；Analyze 落库同步写/清 pixels.json（+镜像字段）；
    `unique_out` 改 create_new 原子认领（取消后重跑不再同名互写）+
    off-by-one（tag-1000）修 + GUI 临时蒙版名加原子计数（pid 撞名）；
    main.rs process_one 改 render→XMP→recipe（完成标记真正最后落——
    render 失败不再被 resume 永久跳过）。
  - **MEDIUM/LOW 实修**：guard_readonly 根部 `..` 折叠修（"D:/../图库"
    弹掉 RootDir 得盘相对路径绕过守卫——+测试）；recipe.clamp 亮度范围
    NaN 界会 panic f32::clamp 断言→非有限整体丢弃（+测试）；Chat SSE
    finish_reason≠stop 截断显式报错；find_sources 符号链接环 64 层深度
    帽；下载渲染失败即时清 tmp；write_pixel_source 终发失败还原 .bak +
    clear_pixel_source 改 Result 三端处理 + backup 版本号 saturating；
    heal 检测缩图免整幅克隆（61MP 全幅 heal 省 ~183MB 瞬态）；heal 重叠
    斑点 last-wins 注释改真话；xml_escape 补 U+FFFE/FFFF；XMP tint-only
    As-Shot 有损边注释在案；apply_masks 全恒等调整跳过整幅扫描；
    apply_lens_distortion 零尺寸守卫；downscale_f32 0 边夹取（自伤修）；
    模型列表供应商切回 API 自动还原 stock URL。
  - **UX 实修**：●未保存涵盖像素身份（pixels_on_disk 镜像，逐帧零磁盘
    IO）+悬停文案；Tint 滑杆随 Custom WB 门控；四处 Full-res 勾选 RAW-
    only 门控；GUI region 框四角映射（旋转下两对角框错/近退化）；定比角
    把手横向余量夹取；文案真相批（设置保存说明/两处忙碌文案改"✕ 取消
    可停"/Save develop 悬停含母版链接，i18n 双侧 422 键 0/0）。
  - **web 实修**：fill/heal 结果带 (id, selectSeq) 双守卫（A→B→A 同 id
    也不串台）；analyze 守卫补 seq；unhandledrejection 全局兜底（传输失
    败不再永挂"…ing"状态）；fill 提示 OPENAI_API_KEY 必需说法改真相；
    风格索引 build 成功后按钮复活。
  - **驳回/在案**（证据）：NoopOnly 弃校准=d1f6986 设计；fit 不走
    produce_recipe=在案设计；heal/clone 中性基图=b1c f7ce 契约；迁移
    TOCTOU/非UTF8键/中断残留=v0.13 取舍；CLI heal 固定名=交付物语义；
    404|422 协商=v0.14.3 裁定（含结构化归因门）；600s 下限 env 旋钮语义；
    claude 验证器 output() 双管并读无死锁（std 语义）；crop_imm 截断；
    批量 3 线程池=v0.11 设计；茶隼级微项（trf 占位符注入/同 mtime 缓存
    陈旧）record。**立项（UX/内存遗留）**：export_mask_png UI 线程编码
    （8192 卡顿）；几何中笔划降采样重映射；窄面板换行批（曲线 160px/
    蒙版工具条/反推行/状态栏●溢出）；AI region 覆盖框几何映射显示；
    Before 缺镜头校正对比；px 重解码烘焙变体文案；flex_size 独立取整可
    破 3:1；segment.py float32 全幅蒙版；denoise sidecar 不可中断+
    torch.hub 代码未钉版本（供应链注记）。
  - **死代码清查（用户令，五角度多重确认）**：①编译器 warning-clean=
    私有/pub(crate) 零死代码 ②163 个 lib pub 项全库交叉引用零未引用
    ③Cargo 18/18 依赖在用 ④web 46 个 JS 定义零死 + i18n 450 条目零孤儿
    （28 条经常量表间接存活逐一核实）⑤python 11 个 def 零死。结论=**无
    可删项**（本会话唯一转死的 composite_region 已即时 cfg(test) 处理）。
  基线 137 lib + 9 gui（NaN 范围/根部 `..` 两用例并入既有测试），clippy
  双零，i18n 422 调用点 450 条目 0/0，web JS node --check 通过。

- **在案后续工作全清批（2026-08-03，用户令"把所有后续工作推完"）**——
  此前三个 backlog 池（v0.14.1 内存专项 4 项 + v0.14.2 待办 2 项 +
  32 路舰队立项 7 项）一次性全部落地：
  - **像素修饰变体关联持久化**：新 store 边车 `pixels.json`
    （store.rs `write/read/clear_pixel_source`，库外 origin 绝对化、库内
    裸名）；Ctrl+S 随 recipe 写/清（gui `save_xmp`，保存文案改真话——
    重新打开会恢复）；打开 worker 连带解码烘焙母版（`OpenedBase` 第 4 元
    `BakedBase`），fresh-open/px 重解码两分支恢复（px 重解码还把 undo 栈
    Arc 重指新分辨率母版）；`nav_stash` 升级为 `StashEntry`（recipe+像素
    身份，wholesale 优先于磁盘）；退出层 pending 携带像素身份、Save-all
    同写；关窗守卫把"像素未保存"也算 unsaved；`forget_open_base` 在修饰
    落地/保存/清除时废弃 LRU。验收：修瑕疵→保存→关→重开，画布仍是修
    过的像素。
  - **GUI worker 取消 + 进度**：状态栏 busy 时对修饰族显示「✕ Cancel」；
    取消=epoch 弃用（`gen_epoch`/`Msg::Retouched(u64,..)` 迟到结果整体丢
    弃、UI 立即解锁）+ generative 线程局部 `WorkerHooks`（fill/reimagine
    传播 cancel 旗标：协商环顶/每 SSE 事件/合成前三处检查点真停流）；
    partial-image 心跳与协商备注经 `Msg::Progress` 进状态栏。
  - **61MP 内存专项（8 子项全落地）**：① render_to_image 加 `max_edge`
    （orient 后 develop 前 f32 降采样 `downscale_f32`，demosaic 后全链在
    工作分辨率跑）——9 调用点接线：GUI 开照 Some(edge)（开照不再全幅驻
    留）、heal/clone/fill/denoise 预览基 Some(2048)、reimagine 高清输入
    Some(2×目标边)、photo_base_knots Some(2048)、web develop_base
    Some(PREVIEW_EDGE)、导出 None；② 生成合成缓冲生命周期分级（mask_full
    即建即弃成 weight、16-bit base 提前 drop、box_blur 复用入参、
    composite_in_place 原位混合）——61MP 全幅填充峰值 ~1.8GB→~750MB，
    输出逐位不变（composite_region 留作 cfg(test) 包装）；③ 色彩取样点击
    改共享覆盖层 `overlay_ref` 缓存（pre 构造与 overlay 对齐：几何字段剥
    除后键相等；miss 时反向预热 overlay），采样 25 像素 get_pixel 替代整
    幅 to_rgb8——2560/4096 预览下点击不再 100-300ms 冻结；④ plan_from_mask
    painted/seen 改 u64 位集（1/8 内存）+ 坐标表改 u32 索引（减半），半
    径数学逐位不变；⑤ fit_zoned 分割输入 ≤2048 帽（CLI 全幅帧不再写
    ~180MB PNG 往返 python；蒙版为归一化数据，引擎任意分辨率重采样）；
    ⑥ CA 逐通道单采样 `sample_bilinear_ch`（逐位同 math，省 2/3 插值
    功）；⑦ 位图蒙版缓存加 256MB 字节预算（原 16 条目可钉 ~1GB）；
    ⑧ 蒙版纹理：笔刷期间脏矩形 `set_partial` 增量上传（无几何常见路径，
    原每帧全幅克隆+重传；8192 预览 ~270MB/帧）+ 有几何时中笔划 120ms 节
    流重建。
  - **web 区域选择几何逆映射**：GUI 的 view→original 复合映射下沉
    render.rs 唯一实现（`view_to_original_norm`/`original_to_view_norm`，
    GUI 变薄封装+往返测试）；analyze 带 region 时 web 端随发 `view`（取景
    配方），serve `region_to_original` 逐角 un-crop→un-rotate→正向畸变映
    射回原始帧再折入提示词（源尺寸不可读时降级为原行为）。
  - **杂项**：CJK 字体候选表 cfg 分平台（macOS PingFang/Hiragino、Linux
    Noto 各发行版布局+文泉驿）；设置窗模型列表按抓取源指纹自动失效
    （`models_from_base`，换 Base/Bridge URL 或供应商即清列表回退保底
    项，fetch 起手即清防失败残留）；serve 启动清扫 >1h 的
    `autoshop_dl_*`/`autoshop_mask_*` 残留（实证 Win11+std 打开句柄下
    unlink 本已成功——泄漏面是 AV 短暂独占后的永久残留，Windows 无自动
    %TEMP% 清理器；api_download 撒谎注释同步改真）。
  **Codex 只读复审 12 条**：8 修——#1 noop 配方+烘焙像素被"清除编辑"路
    径吞掉（Critical，真：clear 门补 `origin.is_none()`——修瑕疵后
    Ctrl+S 现在走保存路径写 recipe+pixels）；#2 Some→None 像素转变不算未
    保存（三处判定改双向比较 `read != origin`）；#3+#4 固定名母版被复跑
    覆写 / 取消后立即重跑同路径写竞态（`unique_out` 探测唯一名统一五个
    starter，含 reimagine 收编；999 上限拒绝）；#5 web 中性保存清单补
    pixels.json；#7b Progress 无 epoch（改 `Progress(u64,..)` 门控）；
    #8 pixels.json tmp 名加计数+旧档退位 .bak；#9 边车在而不可用时
    stderr 警告（不再无声回退）；#12 plan_from_mask >u32 索引空间显式拒
    绝。4 条有据取舍：#6 版本快照不含像素身份（v<N> 先于 pixels.json 设
    计，**立项**：vN.pixels 快照+恢复流）；#7a 取消对静默流最迟等到
    600s 静默上限（钩子悬停文案已如实）；#10 库外母版绝对路径不可迁移
    （./out 本为 cwd 产物，复制进库=每保存全幅 PNG 拷贝，代价不值，注释
    在案）；#11 降采样先于显影 ≠ 先显影后缩（有意语义——与 GUI 预览同
    序，CDF 统计与归一化坐标不受影响，注释在案）。
  基线 **137 lib + 9 gui**（+1：view/original 映射往返），clippy
  --all-targets（含/不含 gui）-D warnings 双零，i18n 421 调用点 0 dup/
  0 漂移（+6 键：取消/母版关联/999 上限文案）。已知边界：Generated 变体
  仍不可 Ctrl+S（无参数化配方，Save-all 例外可存）；取消对本地长算
  （python 去噪）为弃用式（子进程不杀）；web/CLI 导出与 web 预览不读
  pixels.json（GUI 级恢复，跨面统一另行立项——web 清除侧已对齐）；版本
  快照不含像素身份（立项见上）。

- **32 路 gpt-5.6-sol 双视角舰队审查 + 修复/打磨批（2026-08-03，已提交
  未推送）**——用户令"整体 GUI 再打磨一轮 + 派 32 路并行检查 debug，我统筹"。
  执行：fleet32.py（10 片 gui + 2 片 web 带 UX 授权，20 单元全库 debug）
  32/32 成功，118 findings（11H/51M/23L/26UX/7info），40.8万 in/9.0万 out
  tokens。逐条对抗核实后落地（要点）：
  - **advisor**：协商状态窄化到 400|404|422（我自己的注释/代码矛盾——401
    也在旧区间里）；SSE 中途读失败（"read AI stream:" 前缀）纳入 <30s 瞬断
    重试（error/response.failed 事件不重试——不重计费真实失败）。
  - **generative**：fidelity/size 协商同样加状态码门（原任意状态可重发）。
  - **render**：amount=0 蒙版整帧空转跳过；inscribed_dims 45° 正方形
    cos2≈0 → NaN → 1×1 修（走半对角支）；apply_lens_geometry 零尺寸守卫。
  - **recipe**：clamp() 补裁剪校验（NaN 丢弃/夹取/排序/去退化 + 测试）。
  - **xmp**：拉直-only 写 HasCrop=True+全幅载体（LR 只在裁剪态下应用
    CropAngle），读方把全幅载体坍缩回 None（往返测试钉死）；xml_escape 先
    剥 XML 1.0 禁用控制字符。
  - **serve**：上传 create_new 原子认领（并发同名不再互相截断）+失败清残
    体；setdir 双锁同持替换（raws/dir 不再错配）；/api/recipe 拒非对象
    JSON（null/数组=损坏而非新 schema）。
  - **store**：migrate 优先迁移根下的栅格（cwd 同名旁观者文件不再被 staged
    删除——数据丢失级）；delete_version 枚举失败即中止（非 NotFound）。
  - **pipeline**：guard_readonly 词法折叠 `..`（"out/../图库" 绕过修）；
    write_recipe tmp 名带 pid+原子计数（web 线程同进程并发）；.bak 还原失
    败写进报错。
  - **main**：批处理 XMP 先写、recipe 后写（完成标记必须最后落）；auto
    输出位深按扩展名如实播报。
  - **retouch**：AI 斑点上限 30 强制截断。config：NaN 风格强度过滤。
    denoise：临时文件全错误路径清理。lensmeta：CA 至少 8 节点成对。
    denoise.py：overlap 随小图收缩（广播崩溃修）。
  - **gui 状态/手感**：分析落库后 ● 基线改画布投影（烘焙变体不再秒亮
    ●）；AI-select 去重双侧剥 stem 前缀；范围蒙版切离 Color 解除取色武
    装；覆盖层失效改"Amount 前任何改动"+键去重；径向中心拖动夹取位移
    （边界不再压扁）；定比角把手取主导轴（纵向拖不再无效）；1:1 两处
    夹到渲染上限 12；As-shot 取消勾选连 Tint 一起归零；busy 期间 ✕ 拦截
    +提示；方向键无选中时从第 0 张进入；Reimagine 999 上限拒绝而非复用。
  - **文案真相**（i18n 双侧）：修饰基图注释改中性显影真相；"streams
    progress" 状态改"窗口保持忙碌"；OPENAI_API_KEY 必需说法补 OAuth 桥；
    Ctrl/⌘+click；曲线帮助不再承诺 XMP 逐位一致。
  - **web**：分页 latest-wins epoch；换目录清撤销栈；selectPhoto 作废全部
    在飞预览（A→B→A）；下载文件名快照；上传直接流 File（省一倍内存）；
    renderAfter/import 补 catch；布局改 flex（换行表头不再把应用挤出视
    口）；设置说明如实（也作用于生成填充）。
  - **驳回**（证据在案）：heal InPlace"二次显影"（基图=中性显影，注释已
    改）；覆盖层"几何双重应用"（develop_preview 无几何段）；粘贴继承源曲
    线（1488 行明文设计）；crop_imm panic（image crate 会截断）；fit 不盖
    戳/retouch 中性基图/store 迁移并发/feather=1（历史裁决在案）；env 覆盖
    低于 600s 下限（旋钮语义如此）。
  - ~~**立项/待办**：像素修饰的变体关联持久化；web 区域选择的几何逆映
    射；Windows 下载临时文件延迟清理；61MP 内存专项扩充；蒙版纹理每帧重
    传；设置窗模型列表/URL 陈旧性；非 Windows CJK 字体。~~ → **全部已随
    2026-08-03「在案后续工作全清批」落地**（见上方首条）。
  - Codex 复审 3 条全修：同进程 tmp 撞名（pid+计数）；上传失败清已认领
    残体；NaN 裁剪穿透 clamp（+测试）。
  基线 136 lib（+2：clamp 裁剪、拉直-only XMP 往返）+ 9 gui，clippy
  --all-targets 0，i18n 416 调用点 0/0。

- **真机反馈批：瞬断重试 + 主动 AI 去噪 + 提示词配按钮 + 双动词分析
  （2026-08-03，60f8444，已提交未推送）**——四项真机反馈一批落地：
  ① 传输瞬断误报根治：真机报"十几秒就弹 600s 无流活动"→ CLI 无窗复现
    **成功**（52.4s 出真配方）证明是连接期瞬断；修复 = <30s 传输失败自动
    重试一次（post_ai_json 与 images/edits 两处），报错文案改用**实测
    耗时**（远早于静默预算 = 连接/握手故障，措辞不再谎报静默超时）；
  ② AI 去噪改主动画布操作：Detail 节「🤖 立即 AI 去噪」对当前变体像素跑
    SCUNet（RAW→中性显影，默认 ≤2048 工作副本，Full-res 可选全画幅），
    走 heal 的 InPlace 基图烘焙管线（可撤销、滑杆继续其上生效）；新 lib
    入口 denoise::denoise_active；导出时去噪开关保留；
  ③ 提示词入口配专属按钮（用户："AI分析按钮得放到输入提示词那里"）：
    AI Analyze 从工具栏移入 AI 区 Direction 正下方；Reimagine 获得自己的
    提示词输入框+同排生成按钮（原偷用顶部 Direction），提取风格现在回填
    Reimagine 提示词；Generative Fill 本就合规；
  ④ 「微调」勾选框废除（预先勾选的隐藏模式）→ 双动词按钮「AI Analyze /
    AI Refine」（Refine 在编辑中性时禁用——中性时微调即分析）；
    start_analyze(refine: bool) 显式传意图，模式字段删除。
  基线 134 lib + 9 gui，clippy --all-targets 0，i18n 414 调用点 0/0。
  同日随后：32 路 gpt-5.6-sol 双视角舰队（10 片 gui+2 片 web 带 UX 打磨
  授权，其余 20 单元全库 debug）审查本提交树——结果与修复批见下一条记录。

- **v0.14.3 RELEASED（2026-08-03）**——内容 = 全 AI 调用流式化批
  （5ecbbdb，下条）。发版细节见 git tag v0.14.3 与 GitHub Release 页。

- **全 AI 调用流式化 + rationale 失实修复（2026-08-03，v0.14.3）**——
  真机第三次撞总死线：/responses 提案（推理级视觉模型）跑穿 360s →
  回退启发式基线 → 真机验证器判 Revise 并点名"rationale 与数字不符
  （说 +0.0EV/0/0，配方是 -0.06EV/-20/+4）"。两个根因两治：
  - **超时类**（v0.14.2 images/edits 方案泛化到全部文本 AI 调用）：
    advisor::post_ai_json 统一入口——注入 stream:true 走
    post_with_stall_timeout（预算=静默上限，下限 STREAM_STALL_FLOOR=600s，
    因推理期可合法静默）；Responses 族对未设 reasoning 的请求加
    `reasoning:{summary:"auto"}`（SDK 实证参数+
    response.reasoning_summary_text.delta 事件）——推理期间有真心跳；
    协商降级各至多一次（400..=422 且 error.param 实指才降：先摘要后
    stream，拒 stream 回退阻塞+原总死线）；Content-Type 分派容忍
    收下 stream 却回 JSON 的桥。四个调用点收编：propose（openai.rs）、
    describe_style、api 验证器（openai_verify.rs, Chat 族按 index==0 选
    choice 拼 delta 重组阻塞形状）、detect_spots（retouch.rs）。
    共享 SSE 分帧器 for_each_sse_json 下沉 advisor（generative.rs
    read_sse_image 重基于它——全库单一分帧实现）；error_blames_param
    结构化归因（param 优先，无 param 只认带引号提及——"upstream
    error" 不再误触发降级，generative 的 stream 守卫同步加固+状态码门）。
  - **rationale 失实类**：heuristic.rs rationale 移到 clamp+temper 之后
    （temper 软压会改所引数字：-70→-60 有测试钉死）；pipeline.rs 风格
    蒸馏真改动了配方才在 rationale 追加披露行并重验（无变化=不披露
    不重验——反向失实也堵住）。真机判词三条的归因：置信 0.4/过保守
    = 启发式回退的正常面貌（流式修复后 AI 视觉应能跑完，回退变罕见）；
    数字不符 = 上述两处失实，已根治。
  - Codex 只读复审 4 条全处置：#1 静默推理间隙→摘要流+600s 下限；
    #2 "upstream"子串误伤→error_blames_param+状态码门；#3 choices
    位置≠身份→index==0 选择；#4 无变化也披露→pre_blend 比对门。
  基线 134 lib（+5：SSE chat 重组含 index 路由/Responses completed/失败
  事件三例/error_blames_param 五例/heuristic tempered rationale）+ 9 gui，
  clippy --all-targets 0。真机验收点：分析不再"Heuristic baseline
  (…timed out…)"回退（长推理提案能跑完）；若真回退，rationale 数字与
  配方一致；风格蒸馏参与时 rationale 尾部有披露行。

- **v0.14.2 RELEASED（2026-08-03）**——内容 = 修饰流式化批（e729e6b，
  下条）。发版细节见 git tag v0.14.2 与 GitHub Release 页。

- **修饰流式化——images/edits 超时根治（2026-08-03，v0.14.2）**——
  真机第二次撞 images/edits 总死线（300→600s 重校准后，gpt-image-2
  quality=high ~8MP 仍超时）。根因 = 固定总死线对服务时长无上界的同步生成
  端点本质不适配：同一把刀同时杀健康长生成与僵死连接。根治 = **活性与时长
  分离**：
  - `advisor::post_with_stall_timeout`（src/advisor/mod.rs）：只设
    timeout_read/timeout_write（默认 600s）、不设总死线——每次 socket 读
    （等响应头/等下一分片）限时，健康长流无时长上限；语义对 ureq 2.12.1
    源码实证（stream.rs:436 无总死线时响应头等待用 timeout_read；
    response.rs:364 body 读重设同值直至连接回池）。
    AUTOSHOP_HTTP_TIMEOUT_SECS 仍统一覆盖（流式下=静默上限，非总时长）。
  - `call_images_edit`（src/generative.rs）：首选 `stream=true` +
    `partial_images=3`（对 openai-python SDK 类型定义实证：参数存在、取值
    0–3；事件 image_edit.partial_image / image_edit.completed，b64_json 与
    usage 在事件**顶层**）；按响应 Content-Type 分派（text/event-stream →
    SSE 解析，否则原 JSON 路径——服务器收下 stream 却回 JSON 也不会错解）；
    400 拒 stream/partial_images 时按本文件既有协商模式回退一次阻塞
    POST + 600s 总死线（IMAGES_EDIT_TIMEOUT_SECS 降为回退路径）。
  - SSE 解析按规范分帧：多行 `data:` 以 \n 拼接、空行=事件边界、EOF 冲刷
    未终结事件、[DONE] 哨兵忽略、512 MiB 流量上限防无界单行吃内存；error
    事件即刻报错；流收尾无 completed 时报错含已收分片数。partial 分片在
    stderr 打心跳行（"partial image N received"）。
  - 400 参数归因优先结构化 `error.param`（子串匹配仅作无 param 桥的回退）
    ——避免 size 报错文案含 "streaming" 字样时错怪 stream 参数。
  - GUI 状态文案两处 "~15-40s"/"~15–60s" 改为流式+分钟级真相（i18n 双侧
    字节同步）。
  - Codex 只读复审 7 条处置：#2 SSE 分帧 / #3 内存上限 / #4 参数归因已修；
    #1 涓滴字节可无限续命 = 不活超时语义的接受面（自配 API 端点不设恶意
    威胁模型；DNS 解析不可中断为 ureq 2.12.1 既有性质，旧总死线路径同样
    如此，无回归）；#5 三旗全拒最多 4 次 POST = 单调协商成本，接受；
    #7 GUI busy 在健康长跑期间无界禁用按钮且无取消途径 → 立项待办。
  - ~~**待办（未做，用户点单再做）**：GUI 生成/修饰 worker 取消按钮；分
    片进度透出 GUI 状态栏。~~ → **已随 2026-08-03「在案后续工作全清批」
    落地**（见上方首条）。
  基线 129 lib（+5：SSE 分帧×3、双形提取、EOF 冲刷并入）+ 9 gui，clippy
  --all-targets 0，i18n 407 调用点 dup/漂移 0。真机验收点：修饰/AI 生成在
  quality=high 大图上不再 ~10 分钟超时（console 见分片心跳行）；僵死代理
  仍在 ~600s 静默后报错，且报文说明是"流静默"而非总死线。

- **v0.14.1 RELEASED（2026-08-03）**——内容 = 64 路舰队审查修复批
  （7a061f4，下条）。发版细节见 git tag v0.14.1 与 GitHub Release 页。

- **64 路 gpt-5.6-sol 舰队审查 + 修复批（2026-08-03，v0.14.1）**——
  用户令"派遣64路并行gpt 5.6 sol进行代码检查"。执行：/v1/models 实证
  gpt-5.6-sol 存在→代码库切 64 审查单元（gui 19 片/render 9 片/其余按行
  数成组，±30 行上下文+项目契约头）→8 并发扇出（in 442,720/out 117,184
  tokens）→**64/64 成功，152 findings（13H/79M/49L/11info）**→本席逐条
  对抗核实（高危 13 条全判：7 实 6 驳，驳回依据=契约/egui API/已录取舍；
  中危抽样亲读）。修复落地（本批）：
  1. **web XSS**：图库 stem 经 escapeHtml；verdict decision 白名单入
     class 属性；escapeHtml 补引号转义。
  2. **XMP 注释消毒**：rationale 的 "--"/尾 "-" 换 U+2011（含 -- 的
     rationale 曾使整个边车 XML 非法）。
  3. **APEX 光圈**：FNumber 缺失时 ApertureValue 按 2^(Av/2) 换算（原
     直接当 f 值）。
  4. **serve**：下载改文件流式响应（原 fs::read 整个 ~366MB TIFF 进内
     存）；上传 500MB 上限（原无界 read_to_end）+ 同名避让 "name (2)"
     （原静默覆盖）；StyleIndex::save 原子发布（tmp+rename）。
  5. **GUI**：范围蒙版任何改动即刷覆盖层（原只有几何控件刷）；
     OverlayKey 补 profile 畸变/CA 开关（原切开关复用旧 warp）；粘贴带
     出处（copied_from）——贴回剪贴板来源照片保留其位图蒙版（原自贴也
     被剥）+ 打开照片粘贴后 resync；拖放受 confirm_quit 门；CA 回填查
     数据对（有红无蓝时也回填）。
  6. **杂项**：write_recipe 旧档改 .bak 退位+失败回滚（原删旧后 rename
     失败即丢权威档）；claude 验证器子进程 300s 硬预算+杀死+可
     AUTOSHOP_HTTP_TIMEOUT_SECS 覆盖（原 output() 可永久挂起）；
     delete_version 先扫栅格后删配方（原栅格删除失败被吞且不可重试）。
  7. **python 边车**：segment.py 先低分辨率 softmax 选天空通道再单通道
     上采样（原 ~150 类全上采样，61MP 下 ~36GB 必 OOM）；denoise.py
     拒绝 float 输入（原静默按 8-bit 毁图）+ alpha 保留 + tile/overlap
     守卫 + .part 下载临时名带 pid（并发竞态）。
  经读码**驳回不修**（除高危 6 条外）：HSL schema minItems（OpenAI
  strict 模式不支持会 400，代码注释已录）；config 空串键（Settings UI
  契约=空即保留）；eval 锐化 ×1.5（与 xmp ×2/3 互逆，正确）；egui
  drag_delta 累加（API 为本帧增量）；undo 24GB（基图为预览分辨率）。
  ~~**记录为内存专项待办（未做）**：非全分辨率修饰先全幅显影再缩；生成
  合成全幅多缓冲分块化；位图蒙版缓存改字节预算；开照全幅驻留优化。~~
  → **已随 2026-08-03「在案后续工作全清批」落地**（见上方首条）。验证：**124 lib + 9 gui** 全绿、clippy
  --all-targets 0、i18n 407 键 0/0/0、双 exe（gui 34967155 / cli
  26888658）。

- **v0.14.0 RELEASED（2026-08-03）**——自 v0.13.0 后的全部 15 提交：
  修饰超时 600s（8f62b8a）、相机基调批次（b1cf7ce）、Reset=刚打开状态
  （d1f6986）、LR 对标缺陷修复批（f0f96b4+c604477）、分析链超时重校准
  （c111ed0）、手感批（12611db+1807631）、自动镜头校正（d54de4b+
  debde8f）、GUI UX 批（b8ff358+7d823eb）+ 各批 ROADMAP 文档提交。
  发版细节（tag、资产字节）见 git tag v0.14.0 与 GitHub Release 页。

- **GUI 信息架构打磨批（UX batch，2026-08-03，已提交未推送）**——用户报
  "GUI 还是有些乱"，4 区域 UX 盘点工作流（toolbar/develop-panel/retouch-
  tools/visual-consistency，44.8 万 tokens，~80 项分级 findings 全带行号）
  后拍板"工具栏瘦身+归位"。落地：
  1. **工具栏单行化**：第二行整行删除——导出设置六件（格式/长边/锐化/
     质量/色彩空间/AI 降噪）迁入显影面板尾部新 **Export 折叠节**（质量
     滑杆常驻仅禁用=不再回流；AI 降噪 tooltip 补"批量渲染不含"），工具栏
     Export/Download 按钮 hover 动态回显当前交付摘要（export_summary：
     "JPEG · 2560 px · q95 · sRGB"）；AI 三件（Direction 输入框/Refine/
     Style 滑杆）迁入面板顶部 **AI 区**（原 AI verdict 节扩展，一次分析
     的全部输入输出一处齐）；批量进度条迁状态栏（原插在工具栏行首，
     等待时把全部控件右推 160px）；"Autoshop" 字标删除（标题栏已有）；
     Open 忙时禁用（原静默无操作）；Save XMP→**Save develop**（真相：
     非 RAW 根本不写 XMP）；"Export → ./out"→"Export"；撤销/重做 tooltip
     补说明；⿲(U+2FF2 CJK 字形,无 CJK 字体即豆腐块)→◫；⚙ 改开关语义；
     分组用 add_space（separator 换行成孤儿竖线）。
  2. **显影面板重排**（LR 顺序）：AI → 直方图 → Tone & WB → Presence →
     Curves → HSL → Grading →〔分隔〕→ Detail →〔几何〕Lens → **Crop**
     （镜头校正重定义画幅，故 Lens 在前）→〔分隔〕→ Masks → Versions →
     Export；节间 add_space(6)；Grading 的 Blending/Balance 加"全部区域"
     scope 标注（原读作"shadow Blending"）。
  3. **工具武装根治**：新 `disarm_tools()`/`tool_armed()` 取代 **14 处
     手抄互斥列表**（历史上两个真 bug 都来自抄本漂移；瞬时手势锚
     place_start/crop_drag/mask_drag/paint_last 一并归属）；
     resync_recipe_display 全量禁武装（Reset/undo 不再留半武装工具）；
     **range_picking 随蒙版选中变化清除**（原选中换行后取色仍写旧蒙版）；
     **进入图章不再清空画笔涂抹**（原 clear_mask 无撤销地毁掉为填充/修复
     画的区域；Clear 按钮独占清空职责）；放置类按钮（线性/径向/Redraw）
     改 selectable 武装态+可点击取消（原武装后面板毫无变化）；画布提示
     武装时 **ACCENT 常规对比度**（原 .weak().small() 与帮助文字同级）+
     全部armed 提示补"· Esc 退出"+ 径向放置不再被叫"渐变"+画笔提示改
     "Brush —"前缀（原与闲置态同读"After —"）。
  4. **退出确认层安全**：Esc=取消、Enter=全存后退（原快捷键块被 gate 完
     全失灵、备忘单却承诺 Esc）；按钮重排 [Cancel]…[Discard(警示色)]
     [Save all(PILL)]（原三个同样式按钮毁灭键居中）；列表滚动化+显示
     父目录/文件名（原裸 stem 截断 8 个，同名文件不可分辨）；标题
     "● Unsaved edits" 与状态栏 ● 呼应。
  5. **视觉一致性**：克隆源十字 PILL→ACCENT（双层色规自我违反——金色
     在暖色画面上消失）；修饰区 6 个按钮（含 2 个付费 API、2 个毁灭性
     像素操作）补齐 tooltip；蒙版 🗑 补 tooltip；命名规范："Noise Red."→
     "Noise Reduction"、"brush"→"Brush size"（并入 house slider=获双击
     重置+↑/↓ 微调）、两个"Clear"→"Clear crop"/"Clear brush"、蒙版
     "Temp/Tint"→"Temp shift/Tint shift"（相对量≠全局开尔文）、分级区域
     Title Case（Shadows/Midtones/Highlights/Global）、"＋ Radial"→
     "＋ Radial gradient"、🖊→⎘ 图章双钮同形。
  i18n：+40 新键 −26 死键（审计 407 调用点 0 漏译/0 漂移/0 重复）。
  Codex 只读复审 3 条**全修**（其余检查项全 clean，含 python 块交换无
  丢码、resync 全禁武装的四个调用点逐一判定安全、放置按钮互切正确）：
  ①退出层 Enter 全局吞键会盖过持焦的 Cancel/Discard→仅无控件持焦时触发；
  ②Export 节 Format+Long edge 同行两组合框中文下溢出 320px 面板→一行
  一项；③状态栏批量进度条排在 ● unsaved 前会在窄窗把标记挤出→标记
  最前。验证：**124 lib + 9 gui** 全绿、clippy --all-targets 0、终版
  gui exe 34957433（cli 26875258 字节不变——纯 GUI）。真机验收点：工具栏一行放下；
  导出设置在 Export 节且按钮 hover 显摘要；AI 三件在 AI 区；节序 LR 化；
  武装任何工具有 ACCENT 提示+Esc 可退；退出确认层 Esc/Enter 可用。
  记录未做（盘点在案）：变体条重构/状态栏上下文组/WB Temp 常驻/版本
  时间戳/直方图占位/长帮助文demote/Color-Colour 拼写统一/设置窗标题
  层级/全res三勾合一/工具条画布快捷键(K/W)。

- **自动镜头校正（A 档立项落地，2026-08-03，已提交未推送）**——三批之三。
  技术侦察结论（改变方案）：**Sony ARW 每张自带机内校正三件套**（0x7032
  暗角 / 0x7035 CA / 0x7037 畸变，各 16 节点样条，真机 DSC08276 实测齐
  全）——**不需要 lensfun 数据库**，逐张即本镜头在本焦距/光圈的厂商精确
  profile。公式实证（RawTherapee `rtengine/lensmetadata.cc` 实现 +
  stannum.io/blog/0PwljB 逆向互证，量级自洽）：暗角 gain=1/2^(0.5−
  2^(v·2⁻¹³−1))（v=0→恰 1.0）、畸变 factor=v·2⁻¹⁴+1、CA=v·2⁻²¹+1（R/B
  乘在绿通道映射上）；节点位 (i+0.5)/(n−1)，线性插值。用户拍板：**新照
  默认三项全开**（与机内 JPEG 观感一致）。架构：
  1. **[src/lensmeta.rs](../src/lensmeta.rs)**（新模块）：rawler
     GenericTiffReader 提取 + 换算为引擎空间 f32 因子；计数守卫（畸形→
     该分量缺失，绝不报错）；真值单测（A7RIV 实测向量：角落暗角 1.4249/
     畸变 0.9478）+ 常驻真机探针 probe_real_lens_metadata（env 门控）。
  2. **recipe.LensProfile**（引擎专用，不进 XMP）：三数据组+三开关；旧档
     缺字段→默认全关→**逐字节不变**（硬契约，测钉）；is_noop 视"如盖戳
     态"（全部可用分量开）为校准非编辑——用户拨开关=真编辑，存档/导航
     全程存活；clamp 防御手编档（增益 0.25-4/因子 0.7-1.3/CA 0.98-1.02）。
  3. **引擎**：暗角=apply_develop 0a 段线性光增益（16 节点径向 LUT，与
     手动暗角同骨架）；几何=apply_lens_geometry **一次重采样**合成
     profile 样条（Stannum 填幅 s=边缘 max g）+手动量+CA 分通道（R/G/B
     各自半径采样，非 CA 快路径单采样）；lens_geom_norm/lens_ungeom_norm
     正/逆映射（逆=上升前缀二分——复合映射在强手动桶形下过峰回折，域限
     制到峰前，与 undistort_norm 的 u_max 同语义）。
  4. **盖戳三端**：produce_recipe 尾部 photo_lens_profile（saved-first：
     有存档用其 verbatim 含旧档全关与用户开关，否则 fresh_lens_profile
     全开盖戳）；GUI 开照（OpenedBase 三元组+photo_lens 应用态+新照/仅
     XMP 恢复盖戳，recipe.json 原样）、Reset 重盖（烘焙变体空）、粘贴按
     目标重派生（源的校正数据对异镜头无意义）、分析 refine 剥/结果盖、
     烘焙变体画布剥；web：404/fresh-base 双端点带 lens_profile+客户端
     Reset 同义；GUI 批量渲染无存档 RAW 补戳。
  5. **GUI**：Lens 面板顶部"机内镜头校正"三开关（数据缺失灰禁；legacy
     恢复后拨开→从 photo_lens 回填数据）；蒙版/裁剪坐标映射
     (view_norm_to_orig/orig_norm_to_view/geom_to_view) 全量换 LensArg
     （profile+手动量）走复合映射——蒙版把手/覆盖层/画笔栅格在校正后画
     面上不漂移；ensure_mask_tex 变换键含 profile 开关。
  验证：**124 lib（+4：lensmeta 真值/序列化契约/引擎复合与往返/LRU 载
  荷）+ 9 gui** 全绿、clippy --all-targets 0、i18n 390 键 0/0/0、真机
  lensmeta 探针 PASS（16 节点×3）。Codex 只读复审 5 条**全修**：①web
  预览从不做几何（旧账——手动畸变/拉直也从未进 web 预览面板）→
  api_develop 补齐引擎几何链（镜头几何→拉直，不裁剪与 GUI 预览同策）；
  ②画笔覆盖层经 RGB16 路径 **alpha 被打平**（拉直旧账，profile 默认开
  使其必现：整画布红纱）→ 新 `apply_lens_geometry_rgba` /
  `rotate_straighten_rgba` 保 alpha 双胞胎（框外采样=透明）；③变换键
  粒度 geometry_active 布尔无法区分畸变/CA 组合→键改 profile 畸变
  专项布尔（CA 不移动覆盖层）；④基调估计在未校正中性上匹配已校正的
  机内 JPEG=角落提亮被全局曲线二次吸收→新 `render::estimation_base`
  （暗角校正后 ≤1MP 中性）统一 GUI/pipeline/serve 三处估计基准；
  ⑤is_as_stamped 只查 ca_r——半损档（有红无蓝）误判为盖戳态→改查
  CA 数据对。终版双 exe（gui 34936997 / cli 26875258）。真机验收点：开新 ARW 畸变/暗角/CA 即校正（对比机内 JPEG
  构图应一致）；Lens 面板三开关即时生效；旧存档观感不变；蒙版在开校正
  后的画面上位置正确。未做（记录）：非 Sony 机型（Fuji 0xf00b 等）、
  去紫边 de-fringe、lensfun 数据库路线（Sony 元数据已覆盖用户全部机
  身）。

- **LR 对标·手感批（2026-08-03，已提交未推送）**——用户拍板三批之第二批
  （缺陷→**手感**→A 档自动镜头校正）。纯 GUI（引擎/CLI 零改动，cli exe
  字节不变）。4 组：
  1. **滑杆手感**（`slider_impl` 重构 + `SliderFeel` 类别）：悬停 + ↑/↓
     微调（Shift ×10，LR 方向键语法；←/→ 仍是图库走图；以"无控件持焦"
     为门，文本框打字永不误触）；拖拽步进量化——整数域（±100 族/0-150/
     色相°）snap 1 显示 0 位小数（web 端曾被 "13.4849996…" 撑爆值框的
     浮点噪声根治），EV 类 0.01 拖/0.1 微调，0..1 类 0.001/0.01，Temp(K)
     对数轨微调 ≈当前值 1%（固定步长在对数轨一端亚像素另一端过大）；
     新 `slider_fine`（拉直°：0.1 步进——宽域整数 snap 会毁 0.1° 拉平）。
  2. **快捷键**：R=进/出裁剪（与按钮同禁武装工具）、[ / ]=笔刷大小
     （仅画笔/克隆启用时消费键）、Tab=隐藏/显示两侧栏（LR 语法；会话内
     状态，有意不持久化——重启不见图库读作坏了）；速查表 18→22 行。
  3. **裁剪手感**：边中点把手 4 个（自由比例只动该边；锁定比例按 LR 边
     把手语义在垂直轴居中重导出另一维，越界收缩）；**框外拖拽=旋转拉直**
     （绕框心屏幕空间 atan2，y-down 顺时针正=引擎顺时针°，图像跟手转，
     crop_drag 携起始角）；比例补 4:3/3:4/5:4/4:5（7→11 项）；光标
     语义补边把手/旋转；面板与画布提示改述（含 R 键）。
  4. **HSL 8-band 同屏**：波段下拉（一次只见一个 band）改 LR 混色器布局
     ——Hue/Saturation/Luminance 页签 × 8 波段行全显 + 每行色标
     （HSL_SWATCH 展示用波段中心色；引擎数学不动）；hsl_band 字段改
     hsl_tab。
  验证：120 lib + 9 gui 绿、clippy --all-targets 0、i18n 383 键 0/0/0。
  Codex 只读复审 6 条**全实证全修**（含 egui 源码引证）：①旋转 atan2
  ±180° 支切未归一→跨切 2° 被读成 −358° 撞夹（wrap 到 (−180,180]）；
  ②Frac snap 0.001 被 egui fixed_decimals(2) 的**存值取整**吃掉→snap 改
  0.01 与显示位一致；③Tab 的 egui 焦点遍历在 update 前已定向、consume
  拦不住→defocus_next 下帧 surrender_focus（否则首个控件持焦杀死全部
  快捷键）；④Ctrl+Z 中途撤销后 crop_drag 旧锚回写→resync 清 crop_drag；
  ⑤确认退出层背后 R/Tab/[ ] 仍活→快捷键门加 !confirm_quit；⑥微调直赋
  绕过 egui 取整（13.485→14.485 隐藏精度）→按类小数位 round。终版
  gui exe 34683663（cli 26641585 字节不变）。真机验收点：悬停滑杆 ↑/↓
  微调且整数域显示整数；R/Tab/[ ] 生效且 Tab 不吃后续快捷键；裁剪拖边、
  框外转（跨 180° 不跳）、4:3 可选；HSL 8 行同屏。未做（记录）：颜色
  分级 2D 色轮、TAT 图上拖拽调整、面板记忆折叠。——用户真机报分析回退启发式基线：「AI vision unavailable:
  http transport: …/v1/responses: Network Error: timed out reading
  response」。回退与披露本身按设计工作（heuristic.rs 带原因兜底）；根因
  与 8f62b8a（images/edits 300→600s）同类：`/responses` 各调用点预算按
  前推理模型时代校准（提案 120s / 风格描述 90s / 验证 60s），直连
  api.openai.com 跑推理级视觉模型（高细节图 + strict schema）的健康请求
  被客户端中途掐死。修复（[src/advisor/mod.rs](../src/advisor/mod.rs)）：
  ①预算常量化重校准 `PROPOSE=360s / STYLE=240s / VERIFY=180s`（类别注释
  记录实证依据；`AUTOSHOP_HTTP_TIMEOUT_SECS` 仍可全局覆盖）；②中央
  `transport_error` 助手——"timed out" 传输错误一律追加可操作提示（默认
  时限数值 + 覆盖旋钮），三个调用点（openai.rs 提案/describe_style、
  openai_verify.rs）统一换用；③顺藤修出同类更重隐患：**retouch.rs
  `detect_spots`（AI 去瑕疵 auto）用的是裸 `ureq::post`——完全无超时**，
  端点僵死会把 worker 永久卡住而 busy 锁死全 GUI——改走
  `post_with_timeout`（共享 PROPOSE 预算+覆盖）+ 同款超时提示。验证：
  120 lib + 9 gui 绿、clippy --all-targets 0、双 exe（gui 34678011 /
  cli 26641585）。真机验收点：重跑分析应不再 120s 掐断（慢也等到 6 分
  钟）；再超时的报错会写明时限与旋钮。

- **LR 对标·缺陷修复批（2026-08-03，已提交未推送）**——用户拍板三批打磨
  （缺陷修复 → 手感 → A 档立项=自动镜头校正）之第一批；问题源=4 区域 LR
  对标盘点工作流（tone-color/geometry-detail/local-retouch/workflow-ux，
  62.4 万 tokens 实证，锚点逐条 file:line）。7 项全落地：
  1. **曲线端点钉扎**（[src/render.rs](../src/render.rs) `curve_lut` +
     eval.rs 同规则）：曲线未自带端点时补 (0,0)/(1,1)——原先 interp 在首末
     点外夹平，**在曲线上点一下整图变常数灰**（实测复现）；显式端点仍权威
     （提黑曲线不受影响）；AI 产曲线同受益；eval 判分改与渲染同语义。行为
     变化=无端点的旧曲线渲染改变（这正是修复本身，已测钉）。
  2. **Before=画布配方的基调**（gui `set_before` + `before_curve` 字段 +
     update 每帧惰性刷新）：Before 面板/hold-B 原显示无基调中性显影，比
     After 起点暗 0.63~1.42 EV；现新照对比亮基调、旧存档按原调、烘焙变体
     原样，Reset/粘贴/恢复改曲线即自动重建（一次 LUT develop）。
  3. **直方图真相**：256 bin（每 8-bit 码值一格）+ 四通道**共享**纵轴归一
     （原每通道各自归一→相对高度无意义）+ 三角指示阈值对齐 J 覆盖
     （≤1/≥254，原极端 bin 跨 0-3/252-255 会对近削波亮警）；build_preview
     里"judged on EXPORT pixels"谎言注释改为如实（8-bit 预览值、缩放可
     平均掉小面积高光溢出=记录残差）。
  4. **锐化半径随分辨率**（V2_PLAN §4c 落地）：σ=clamp(0.0008·min(w,h),
     0.7,2.0)→box 半径（1280 预览=1，≥约1900 及全尺寸导出=2）；原硬编码
     1px 使预览与 61MP 导出结构性不同。NR 依 §4d 像素尺度**不变**（噪声粒
     度在传感器像素尺度），预览/导出观感差=披露注释。行为变化=大图带锐化
     导出字节改变。
  5. **径向蒙版 feather 补全**：GUI 边缘羽化滑杆 + 内外翻转开关（两者引擎
     一直渲染却均无控件，放置硬编码 0.5）；XMP **双向换算**——写方 ×100
     （旧写方把 0.5 原样给 LR 的 0..100 域=硬边）、读方 >1→÷100、≤1 按旧
     自写保真（LR 真值 Feather="1" 角例记录于注释）；新测试钉双向。
  6. **像素修饰可撤销**：undo 三栈改 `UndoStep{recipe, base(Arc), origin}`
     ——heal/clone/生成填充烘基图现在是一步可撤销（committed 按 Arc 指针
     身份比对；undo/redo 恢复变体像素 + refresh_active_pixels 重建
     before/画布/overlay）。内存取舍：每次修饰在历史里多留一份预览分辨率
     基图（100 步上限自然回收）。
  7. **web**：新 `/api/fresh-base` 端点 + Reset=刚打开观感（与 GUI d1f6986
     同义；旧服务器/非 RAW 回退保曲线）；9 处中文残留英文化（页面英文-
     only：AI 去瑕疵按钮/状态、修图/分析/图像节标题、popover 三库名）。
  验证：**120 lib（+2 新）+ 9 gui** 全绿、clippy --all-targets 0、i18n
  378 键 0/0/0。Codex 只读复审 4 条：同帧撤销竞态（HIGH，修=修饰落地帧
  `commit_now` 即时入史）、sel_mask 越界（修=resync 过滤，同号异蒙版残留
  记录）、曲线重复 x（注释钉"先到先得"）、LR Feather="1"=1% 角例（记录
  取舍——≤1 判旧自写保真优先）。终版双 exe（gui 34673242 / cli
  26636565，`c604477`）。真机验收点：曲线单点不再毁图；Before 与开照同
  亮度；修饰后 Ctrl+Z 能退；径向蒙版可调羽化。

- **Reset=刚打开状态 + 旧存档提示（2026-08-02，`d1f6986`，已提交未推送）**
  ——真机验收报"点了重置，还是暗的"。根因：用户已编辑照片几乎都有基调批次
  **之前**的旧存档（硬契约=缺 base_curve 字段→空曲线→按旧样渲染），而
  Reset 沿用"保画布曲线"语义——旧存档上曲线本来就是空，重置后依旧暗。修复
  （[src/bin/gui.rs](../src/bin/gui.rs) + [src/bin/i18n.rs](../src/bin/i18n.rs)，
  +41/−7）：①开照 worker 估计的基调节点存入新应用态字段 `photo_knots`；
  ②Reset 改为**重盖 photo_knots**（=本照片刚打开的观感：滑杆中性 + 相机
  基调；烘焙变体仍空曲线——其像素已带相机观感）；③打开旧存档照片加一行
  状态提示（"保存于相机基调功能之前——按原样渲染；Reset 可切换到相机基调"，
  EN/ZH）；④Reset tooltip 改述新语义。链路自洽：旧存档上 Reset→Ctrl+S =
  is_noop→中性存档删边车→照片回"新照"，下次打开自动盖戳（用户迁移旧照的
  一键路径）。已知残差：**web Reset 仍保画布曲线**（web 打开旧存档同样暗，
  无提示——待下批）。验证：118 lib + 9 gui 绿、clippy 0、i18n 375 键
  0/0/0、双 exe 重建（gui 34667456 / cli 26645626）。

- **相机基调批次：打开 RAW 不再暗 + 修饰不再切换像素源（2026-08-02，用户
  拍板"根治: 基调+统一"）**——用户报"每次加载 raw 都很暗，点 AI 修瑕疵就恢复
  正常亮度"。根因两层：①中性显影无任何相机基调（rawler 线性→sRGB，真机定标
  比机内 JPEG 暗 +0.63~+1.42 EV，且形状是 S 曲线非单一增益——中间调动 ~3×
  于趾部）；②heal/clone/生成填充非全分辨率基图曾是**相机内嵌 JPEG**，修完
  InPlace 烘焙进画布 = 像素源切换假装"恢复亮度"，且后续编辑/导出落在 8-bit
  相机曲线像素上。方案（[src/recipe.rs](../src/recipe.rs) `base_curve` +
  [src/render.rs](../src/render.rs)）：
  1. **recipe.base_curve 携带式相机基调曲线**（引擎专用，不进 XMP）：分位数
     锚点 luma-CDF 匹配（x=Q_neutral(p), y=Q_camera(p)，p 网格止于 0.98，
     (0,0)/(1,1) 钉扎；两侧对称 ≤1024px 缩略+全像素 1024-bin 直方图；同 bin
     分位数均值合并；近恒等→空）。`build_tone_lut` 把它复合在用户色调**之下**
     （final(x)=user(base(x))，LR 的 profile-then-sliders 次序）。**旧存档
     缺字段→空曲线→逐字节按旧样渲染**（硬契约）；fit 产物有意不带曲线（自洽
     完整解）；is_noop/●判定（新 `dirty_vs`）忽略曲线——校准非编辑。
  2. **盖戳权威 = pipeline::produce_recipe**（saved-first：有 recipe.json 用
     其曲线原样含 legacy 空，否则 photo_base_knots 新估计）——GUI/web/CLI 三端
     同一权威；GUI 开照（新照+仅 XMP 恢复盖戳，recipe.json 原样）、web 404/
     XMP 路径带节点（fresh_base_knots 复用 develop_base 缓存）、粘贴每目标
     解析（烘焙目标清空/有存档用其曲线/否则承源）、GUI 批量渲染无存档 RAW 补
     戳、GUI/web Reset 保画布曲线（GUI Reset 语义已被上条 `d1f6986` 取代为
     重盖开照节点）、烘焙变体上 Analyze 画布剥曲线（持久化仍全）。
  3. **修饰基图统一**：heal/clone_stamp/生成填充 RAW 基图一律引擎中性显影
     （full_res 全尺寸，否则 ≤2048px 缩略）——修完与画布同色调链，亮度跳变
     根治。代价：非全分辨率修饰也要 demosaic（clone 从即时变数秒）——已记录。
  4. **decode::embedded_preview**：相机自带渲染三级提取（preview→thumbnail→
     full_image）。rawler 0.7.2 源码验证：ARW 解码器**只实现 full_image**
     （JPEGInterchangeFormat 内嵌全尺寸 JPEG 提取，绝非 develop；默认实现
     Ok(None)）——A7RIV 全靠第三级；decode.rs 两处"full raw render"旧注释
     已纠正。
  5. 审查：对抗工作流 26 代理（4 维度 find + 逐条对抗 verify）22 findings
     （14 confirmed 含 2 BLOCKER：估计器直方图空洞→~30 级色调断层/暗片高光
     整段钉白（已用真实代码复现）、中性清存后 ● 永久点亮、CLI 写方 recipe
     永久钉暗、粘贴无 is_raw 门等）+ Codex 复审 5 条（烘焙变体双重上调、web
     新照暗突变、步进混叠、同 x 取低偏置、双 develop 成本）——全部修复或记录。
  6. 有据取舍：analyze-after-fit 继承 fit 的空曲线（saved-first 一致性优先）；
     auto/batch/GUI 批量对无存档 RAW 多付一次 develop（正确性优先，已披露
     warning）；web retouch 面板残差；Reset 在变体上保画布曲线语义。
  验证：**118 lib + 9 gui** 全绿（+5 新测试：复合/端点钉扎、空洞无平台无钉白、
  尖峰均值合并、旧档序列化/is_noop/clamp、LRU 载荷）+ 常驻真机探针
  `probe_real_raw_base_look`（AUTOSHOP_PROBE_RAW 门控：DSC08530.ARW median
  中性 0.326 → 带基调 **0.497** vs 相机预览 **0.495**）；clippy 0；i18n 374
  键 0/0/0。真机验收点：打开任意新 RAW 应即近相机 JPEG 观感；点修瑕疵前后
  亮度不再跳；旧已保存显影观感不变；Reset 不再变暗。

- **修饰(images/edits)超时预算重校准（2026-08-02）**——用户真机报
  「修饰失败: transport: …/images/edits: Network Error: timed out reading
  response」。根因：直连 api.openai.com（image_provider=api，非 8317 桥）
  跑 gpt-image-2 + quality=high + 像素预算默认 8 294 400（API 上限），
  生成时长超过 [src/generative.rs](../src/generative.rs) 里按 gpt-image-1
  时代（60-120s）校准的 300s 总预算——正常成功中的请求被客户端掐断。
  修复：预算常量化 `IMAGES_EDIT_TIMEOUT_SECS = 600`（每次 400-重试各享
  全额），transport 报错含 "timed out" 时附加可操作提示（默认时限数值 +
  `AUTOSHOP_HTTP_TIMEOUT_SECS` 覆盖 / 降 `AUTOSHOP_IMAGE_QUALITY` /
  `AUTOSHOP_IMAGE_MAX_PX` 提速）。诊断副产物（环境事实，未改代码）：
  中央 `%LOCALAPPDATA%/autoshop/autoshop.local.json` 尚不存在，实际生效
  的是 cwd 回退的 `D:/Projects/Autoshop/autoshop.local.json`（设计内：
  下次在设置 UI 保存即写中央文件）；OPENAI_API_KEY 现存在于 .env 与
  用户级环境变量。

- **第二轮 debug + UI 打磨批次（2026-07-25，已提交未发布——待真机验收）**
  ——用户令"再进行一遍debug+UI前端打磨"（中途不弹窗）。方法：6 维度工作流
  （store 集成/GUI 状态机/GUI 打磨/web/双语文案/跨端契约）+ 按维度对抗验证
  + 完整性批判 → 61 确认 + 4 批判补充，去重后 ~35 项全部处置；Codex 复审
  再出 6 条（3 MAJOR）全修。要点：
  1. **备份门下沉 lib**（store::backup_saved_develop）：复制式快照（工作
     recipe 不动，可在操作前打快照）+ **栅格版本化**（v<N>.<name>.png 冻结
     引用，快照不再被后续 fit 覆写栅格改写观感；复制失败=整个备份失败并
     回滚——绝不假称有备份）；三端统一：web /api/analyze 与 GUI 同门（拒
     写时 200+warning），CLI 侧 StyleIndex::save 拒绝空索引覆盖好索引；
     delete_version 连冻结栅格一起清。
  2. **GUI 数据安全**：Fitted 带 persisted 旗标（备份门拒绝时不再假清
     ● 基线/nav_stash）；zoned fit 快照提到分割前+门拒绝时跳过 zoned
     （保护已保存栅格）；fit 的 XMP 失败不再吞掉已成功的 recipe 写入；
     Analyze 拆分 recipe/XMP 写（基线只随 recipe）+失败 toast；粘贴到
     打开照片全成功后同步基线（假 ● 修）；save_version 从磁盘取号（缓存
     失刷不再覆写自动备份）；**关窗拦截**（✕ 有未保存→应用内 保存全部/
     放弃/取消 层，不弹系统窗）；crop 拖拽/Clear 置 dirty（直方图/削波
     不再滞后 + 在飞帧丢弃后重派）；resync 清 range_picking/placing_mask
     等武装工具索引；AI-select 去重按栅格名（stem 前缀感知）并复活旧引用。
  3. **GUI 打磨**：AI verdict/rationale 移到显影面板顶部（Accept 平静色/
     其余警示色）；设置窗新增「显影库」区（根路径展示 + **从旧 ./out 导入
     显影**按钮→worker 迁移全图库）；版本行 🗑 删除；方向键走图自动滚动到
     选中缩略图；Temp(K) 对数刻度；Tone & WB 补 ● 活动点；蒙版重命名缓冲
     （一次改名=一步 undo，跨行安全提交）；Heal 全分辨率 tooltip/质量下拉
     悬停+译；Download… 过 guard_readonly；缩略图缓存并入 store_root
     （AUTOSHOP_DATA_DIR 生效）；设置窗标题/模型状态/Before 标签等全译。
  4. **web**（子代理）：分析结果串照片根治（id 守卫）+selectPhoto
     latest-wins；非 ASCII 文件名不再 panic（RFC5987）；损坏 recipe 422+
     前端兜底；web Reset+Save=删四处存档（与 GUI 同义）；blob URL 回收；
     文本框内 Ctrl+Z 不再回滚配方；分析/保存后徽章即时刷新；popover 展示
     显影库路径+风格索引实际文件。
  5. **migrate 加固**：进程内 memo（打开不再每次全枚举 ./out）+ legacy
     多根探测（env AUTOSHOP_LEGACY_OUT → cwd → exe 目录，升级用户换目录
     启动不再"丢"旧编辑）+ 显式导入 API。
  6. **文案真相**：所有仍称显影态在 ./out 的字符串（GUI 5 处/CLI help 与
     -o 尾注/web popover/README/ARCHITECTURE --bare 陈旧调用）全部改为显影
     库表述；ZH 修正：变体≠版本、双击恢复默认值（非归零）、局部蒙版、
     全分辨率（非全画幅）、gitignored 措辞、sidecar/边车保留给 XMP。
  基线 **113 lib + 9 gui**（+2 serve 编码测试）、clippy 0、i18n 374 键
  0/0/0。已知取舍：memo 使"会话中途出现的 legacy 文件"延至下次进程或
  手动导入（文档化）；web 陈旧未保存分析提示只改文案不缓存提案；
  serve `image_oauth_supported` 字段保持 false（web 端确实不可配置）。

- **边车中央库批次（2026-07-24 实现，已随 v0.13.0 发布 2026-07-25）**——
  用户拍板"中央库+路径键"后实现。显影**状态**（recipe.json / <stem>.xmp /
  v<N> 快照 / 蒙版栅格）从 cwd 相对 ./out 的裸 stem 键迁至
  `store::store_root()`（`AUTOSHOP_DATA_DIR` env → `%LOCALAPPDATA%/autoshop`
  → temp）下 `develops/<stem>-<fnv1a64(绝对路径小写)>/`，同名异目录照片
  不再互踩、换目录启动也能找到编辑；**导出成品图（developed/retouch/heal/
  clone/preview/matched/style.txt）留在 ./out**（交付物）。要点：
  1. 新模块 `src/store.rs`：路径键（FNV-1a 64 手写钉死——DefaultHasher 跨
     Rust 版本不稳，会孤儿化整库）、`recipe_target/xmp_target/version_target/
     raster_target/style_index_path/settings_path`、`has_develop`（中央∨
     legacy 存在性，供 badge/web 列表/CLI batch）、`migrate_legacy`（打开
     即迁移：解析 recipe 连带栅格搬家并把引用改写成裸名；解析失败原字节
     搬（Unreadable 契约保持响亮）；跨卷 rename 失败退 copy+delete；央本
     已存在则不覆盖）、`resolve/relativize_mask_paths`（读时锚定 recipe
     所在目录/写时收敛裸名——recipe 内栅格引用相对化，显影目录可搬迁）。
  2. pipeline：`write_recipe` 默认写央本+落盘前 relativize 副本+source.txt
     面包屑；`xmp_target` 重指向央本（全部调用点自动跟随）；`guard_readonly`
     放行 store root；风格索引读取央本→legacy 回退。
  3. GUI：read_saved_develop 顶部 migrate_legacy + 央本→legacy 双读（kind
     注明 "(legacy ./out)"）+ resolve；backup_saved_develop 比较前先
     resolve（否则带栅格的重写永远"不同"→假快照）；版本快照/列表走
     store；中性存档删除**四处**（央本+legacy 的 recipe+xmp，legacy 残留
     会经回退复活"已清除"的编辑）；批渲染读央本→legacy；badge=
     has_develop；分割/反推栅格写入显影目录。
  4. Web/CLI：/api/recipe 迁移+双读；/api/develop|export|download 渲染前
     resolve（api_recipe 原样透传裸名）；风格索引构建/信息走央本；
     `match --zoned` 栅格+canonical 走 store（fit_zoned 不建目录，调用点
     补 ensure_parent）；`apply` 按 recipe 所在目录 resolve；`batch` 的
     pending 判定=has_develop（旧库不再被重复计费重分析）。settings
     （autoshop.local.json）移央本+旧 cwd 文件读回退。
  5. 测试 111 lib（+4：photo_key 消歧/FNV 参考向量/resolve-relativize
     往返/migrate 全套）+9 gui（sidecar 优先级测试加 legacy 迁移断言）。
  Codex 只读复审 9 条：**3 条已修**（迁移改"先复制栅格→tmp+rename 发布
  recipe→成功后才删旧"失败全回滚；backup_saved_develop 改 Result——快照
  失败则 AI 分析/反推**拒绝覆盖**显式保存并 toast/note 说明；write_recipe
  改 tmp+rename 发布，崩溃不再留半截真源 JSON）；6 条记录为取舍——并发
  迁移竞态（同源幂等，见 migrate_legacy doc）、中断迁移（逐文件可续跑）、
  相对 AUTOSHOP_DATA_DIR（高级用户显式覆盖自担）、路径拼写身份（symlink/
  UNC 不 canonicalize——其自身失败模式更多，source.txt 供诊断）、64 位
  FNV 碰撞（需 stem+哈希同撞，个人库量级 ~1e-10）、非 UTF-8 stem lossy
  （Windows 全 Unicode，风险极低）。其余已知取舍：迁移静默成功（状态一致
  无需打扰；失败时 kind 带 "(legacy ./out)" 可见）；serve.rs
  `out/imported` 上传目录留 ./out（属输入暂存）；fit_zoned 单测仍写测试
  自有的 ./out 文件（测试专用路径，非生产写点）。

- **全量 debug + 协同性审计批次（2026-07-24，已随 v0.12.0 发布）**——用户令
  "全量debug+打磨使用体验，同时检查各个功能之间的协同性"+报"蒙版按钮
  点击变移动"。方法：6 维度并行审计工作流（指针状态机/异步预览/边车
  持久化/蒙版渲染语义/偏好流程/跨面协同）+ 按维度对抗验证 → **77 项
  findings 全部 CONFIRMED**；修复分四批（gui.rs 本体串行保协同性；
  render.rs、serve.rs+web、pipeline/main/advisor/segment 三组互不相交
  文件由隔离子代理执行）。基线 **107 lib + 9 gui**（+5 新测试），
  clippy(--all-targets --features gui -D warnings) 0。要点：
  1. **蒙版行点击=选择恢复**（用户报症）：整行曾包在 dnd_drag_source
     里（drag 交互层吃掉点击、微动即浮行）→ ☰ 专用把手当唯一拖拽源，
     行主体纯 selectable_label，drop 落点区=整行（gui.rs 蒙版列表）。
  2. **持久化统一契约**：recipe.json = 任意源类型的唯一真源（Ctrl+S 对
     PNG/TIFF 也写 recipe.json，仅 XMP 限 RAW）；中性配方存档=删除边车
     （清 ● 徽章）；程序性写入者（AI 分析、反推）覆盖前自动把既有存档
     备份为 v<N>（backup_saved_develop；显式 Ctrl+S 直接覆盖）；GUI 分析
     现与 CLI/web 一样持久化；web /api/xmp 双写 + /api/recipe 回退 XMP；
     CLI match 补写规范边车、analyze -o 时 XMP 跟随同目录。损坏的
     recipe.json 走 SavedDevelop::Unreadable **响亮降级**（toast+状态），
     中性边车 NoopOnly 不再谎称"已恢复"。
  3. **未保存保护**：saved_recipe 基线 + 状态栏 ● 未保存标记 + 导航
     nav_stash（切照片把未保存画布按路径暂存本会话、回来即恢复并提示）；
     批渲染对打开中的照片改用活配方（与 paste 同规）。
  4. **指针/工具状态机**：Esc/中断后残留 crop_drag/mask_drag/paint_last
     全部清理（无拖拽即无抓取原则）；快速抓取用 press_origin 命中（egui
     drag_started 有 ~6px 阈值）；小蒙版把手最近者优先取边缘；删除/拖放
     /⬆⬇ 后 range_picking/placing_mask 索引统一经 remap_mask_indices
     重映射；Paint 复选框补全模式互斥；画笔支持单击点涂+双击缩放在工具
     态被禁；削波层以当前旗标为准（在飞帧不再覆盖 J 切换）；AI 区域框
     常显（不再被工具/把手悬停藏掉）。
  5. **引擎蒙版语义**（render.rs，子代理）：位图栅格缺失＋invert 曾整幅
     全强度应用→现整体跳过（含 coverage 一致）；feather=0 椭圆边界 NaN
     （黑点）→ ramp 钳制硬边；roundness 实测无法定契约（用户 160 个 LR
     边车 201 个径向全 0）→ 显式记录为 no-op + 钉死测试。反推 sky 栅格
     改名 mask-zone-sky（GUI+CLI），不再与「AI 选天空」的 mask-sky 相撞
     互删；重跑分割去重同栅格蒙版；批量粘贴剥离位图蒙版（带提示）。
  6. **Web 协同**（serve.rs+index.html，子代理）：预览与导出统一解码源
     （develop_base 缓存 + 构建互斥锁）；latest-wins 序号防旧帧回流；
     500 带真实错误文本（tiny_http Drop 空响应根因）；空风格索引拒绝
     覆盖好索引。
  7. **杂项 UX**：B 对比/Space 平移焦点门（打字不再闪原图）；⚙ 重点击
     不再清空表单；模型拉取中重开设置不丢 in-flight；保存后 key 提示按
     解析后配置（env key 不再误报"未设置"）；提供商切换换默认模型；预览
     worker panic 不再误清 busy；变体条 busy 时点击给 toast；保存/版本
     失败走 toast；启动语 i18n；多文件拖放提示；直方图/削波按裁剪窗计算
     （与导出像素契约一致）；straighten 下径向真实轮廓多边形 + 画笔
     overlay 过同一几何变换；XMP 读回"Autoshop N"占位名视为未命名；分割
     蒙版持久名固定英文键（数据不随语言漂移）；F1 双击说明改"重置为默认
     值"；README 图像 OAuth 描述纠正；python sidecar 输出改捕获、错误尾
     部随报错透出（lib.rs sidecar_tail）。
  已知取舍/遗留（勿当 bug 重报）：边车按裸 stem 键控在 ./out（同名异
  目录照片相撞）与 out/settings 的 cwd 相对性=磁盘布局设计题，**用户已
  拍板（2026-07-24）：中央库+路径键**（%LOCALAPPDATA%/autoshop/，按照片
  绝对路径消歧，./out 只留导出成品图，打开时自动迁移旧边车），作为
  v0.12.0 之后的独立批次实现、真机验收后另发；radial 边缘把手在
  straighten 下的手感（引擎正确，显示已用多边形，拖拽轴映射保持原
  语义）；>24MP 缩略图门排队策略未动；web 预览 8-bit 降采样基底与导出
  的残余差异已在 serve.rs 注释记录；badge 仍为存在性检查（开销权衡，
  异常态由打开路径 toast 补偿）；pipeline 融合后复验仍 fatal（子代理
  记录）。

- **AI 分析认证修复（2026-07-17，已随 v0.11.2 发布）**——v0.11.1 过了
  信任门后用户再报 "分析失败: claude exited Some(1): "（冒号后空白）。
  三层根因（全部当日实测复现，`src/advisor/claude.rs`）：
  1. **`--bare` 语义变化**：本机 `claude` CLI 已自动升级 2.1.158→2.1.210，
     新版 `--bare` 的 --help 明文 "Anthropic auth is strictly
     ANTHROPIC_API_KEY or apiKeyHelper via --settings (OAuth and keychain
     are never read)"——即 `--bare` 下验证器**永远不可能**走用户订阅
     OAuth（OAuth 凭据本身健康且自动刷新正常）。A/B 实测：同一调用
     带 `--bare` 报 "Not logged in"（无 key）或 400（有 key）；去掉
     `--bare` 即成功。修复：换成 `--setting-sources "" --strict-mcp-config
     --disable-slash-commands` 三旗标——实测 0 插件启用、0 hook 注册、
     无用户技能、stderr 干净（保住当年 `--bare` 的隔离目的），OAuth 可用。
  2. **环境里的 `ANTHROPIC_API_KEY` 抢占计费**：Windows 用户级环境变量
     里的 key 被 headless claude 优先采用（即使不带 `--bare`、即使用户
     在交互模式里拒绝过该 key），把计费导向余额为空的 API console →
     400 "Credit balance is too low"。修复：spawn 前
     `cmd.env_remove("ANTHROPIC_API_KEY")`（该 provider 设计即 OAuth）。
  3. **错误吞没**：headless claude 失败时通常 stderr 为空、真实错误在
     stdout JSON 信封（`is_error:true, result:"…"`），而 CliFailed 只带
     stderr → 用户只看到 "claude exited Some(1): " 空白。修复：失败路径
     先解析 stdout 信封透出 `result`；解析不出且 stderr 为空时带上
     stdout 前 300 字符。
  注意：此修复要求 `claude` CLI 支持这三个旗标（≥2.1.x；CLI 默认自动
  更新）。机器侧删除用户级 `ANTHROPIC_API_KEY` **救不了** v0.11.1 及更早
  的 exe（`--bare` 下会变成 "Not logged in"）——唯一根治在代码侧。

- **AI 分析信任门修复（2026-07-14，已随 v0.11.1 发布）**——用户报
  "分析失败: claude exited Some(1): Ignoring 3 permissions.allow entries
  from .claude/settings.json: this workspace has not been trusted"。
  根因：`ClaudeProvider::verify` 直接 `Command::output()`，子进程继承
  GUI/CLI 的 cwd；headless `claude` 把 cwd 当工作区，若该目录带
  `.claude/settings.json` 且从未交互接受过信任对话，CLI 报错并退出 1。
  实测复现（2026-07-14）：同一调用在 `D:/Projects/Autoshop` 下 stderr
  逐字节出现该信任报错、在全新 temp 目录下无报错。修复：spawn 前
  `cmd.current_dir(std::env::temp_dir())`（验证器纯 stdin/stdout，
  不触碰文件，temp 目录恒存在且无工作区设置）。全库唯一 `claude`
  spawn 点即此处（`src/advisor/claude.rs`；`denoise.rs`/`segment.rs`
  的 spawn 是 Python sidecar，无关）。

- **性能批次 #3-B：引擎并行化 + 全量 backlog 清零（2026-07-12，已随 v0.11.0 发布 → `e3a4096`）**
  ——把 v0.10.0 记录的 29 项审计 backlog **全部**落地（`b15996c` 引擎 +
  `e2b4b6b` GUI + `73b4f43` CLI/Web/Style，后者由两个 worktree 隔离子代理
  实现、diff 回传合并）。release 基准探针（钉死校验和）**通过**且大幅加速：
  单彩色蒙版 81→44.7 ms/帧、天空+地景对 149→34.5、无彩对 92→25.0
  （相对 v0.8.1 同机基线）。
  1. **引擎 rayon 并行（render.rs，`b15996c`）**：rawler demosaic 本就并行，
     尾部全串行——现在 tone/RGB曲线/HSL/分级/饱和/dehaze/暗角/双蒙版通道/
     unsharp/NR/u16·u8 打包/f32 转换/旋转·畸变重采样全部按行或按像素并行。
     逐像素数学未动 → 逐位不变（模糊通道保持每列运算次序）。新依赖
     rayon（本就在 Cargo.lock 里，rawler 用）+ bytemuck（零拷贝转换）。
  2. **powf 热环 LUT 化**：dehaze/暗角每像素 6-7 次 powf → 共享 4096 项
     传递曲线 LUT 对（同 v0.8.1 色偏增益已验证的亚量化误差包络）+ 暗角
     径向增益 LUT；dehaze 的 airlight 直方图**保留精确 powf**（避免估计
     跨 bin 漂移）。convert_export_color_space：u16 输入 → 65536 项**精确**
     解码表（逐位不变）+ 按值传递（sRGB 恒等与 16 位路径改移动，省 ~366MB
     克隆）+ 行并行。
  3. **内存/访存**：box_blur_v 改行主序 + 每列 running-sum（原列主序在导出
     尺寸平面上全程打到 DRAM 延迟；逐位不变）；orient_f32 bytemuck 零拷贝
     （竖构图 61MP 原付三次 ~732MB 拷贝）；rotate/distort 借用已是 Rgb16 的
     源不再克隆；打开时中性 tone-pass 短路（原对全传感器无条件跑恒等 LUT）。
  4. **GUI（`e2b4b6b`）**：削波开关（J/▲/直方图三角）改用保留的上一帧像素
     即时重建 overlay（原每次开关整幅重显影 100-300ms）；RGB→RGBA 纹理
     转换移入 worker（PreviewDone.after，UI 线程只 tex.set）；**持久缩略图
     磁盘缓存**（%LOCALAPPDATA%/autoshop/thumbs，键=路径+mtime+大小哈希，
     二次会话 ~1ms 回显，>10k 文件按龄一次性后台修剪）；>24MP 烘焙图解码
     单许可门（原 6 并发 60MP TIFF 峰值 ~2GB）；图库纹理超 1500 项按视口
     窗逐出（原 1 万图钉 ~0.7GB）；蒙版覆盖栅格上限 1024px（display-only）。
  5. **CLI/Web/Style（`73b4f43`）**：CLI batch 3 线程有界池（网络与渲染重叠，
     --limit 语义/失败语义/汇总保持，stdout 行原子）；Web /api/develop 与
     preview/thumb 共享 (path,mtime) 键解码缓存（每滑杆手势不再全幅解码）；
     风格索引 4 线程池按槽位写回（exemplar 次序与串行完全一致）。
  基线 **102 lib + 9 gui** 全绿、clippy 0、探针校验和不变。已知偏差（诚实
  记录）：dehaze/暗角像素环走 LUT（8-bit 输出在量化内不变、16-bit 可 ±数
  LSB，同 v0.8.1 色偏先例）；打开路径中性 tone-pass 跳过消除了旧恒等 LUT
  的舍入噪声（更正确，非逐位同旧版）；覆盖 overlay 超 1024px 用降采样参考。
- **边车恢复 + 响应性快赢（2026-07-12，已随 v0.10.0 发布 → `c312a9f`）**——用户报
  "图库显示 ● edited 但打开后不加载 XMP"。根因：徽标查 ./out 边车
  （recipe.json‖xmp，gui.rs `gallery_panel`），而打开路径从不读它们
  （`Msg::Opened` 新开分支一律 `EditRecipe::default()`）；且 GUI 自己的两条
  保存路径（Save XMP 按钮、反推 worker）只写 XMP——位图蒙版/重着色增益
  根本进不了经典 XMP，反推结果关掉即丢。三层根修（`6957897` lib + `3124596` gui）：
  1. **XMP 读取器 `xmp::xmp_to_recipe`（xmp.rs，与写入器同文件）**：逐字段反演
     `recipe_to_xmp`——全局滑杆/HSL/分级/曲线/裁剪拉直/暗角畸变/参数化蒙版
     （线性+径向，含范围蒙版；位图蒙版与写入器同规则跳过）。**溯源规则**：
     As-Shot 的 Temperature/Tint 是相机值不是编辑，仅 `WhiteBalance="Custom"`
     才导入（Tint 另认我们自己的 `x:xmptk="Autoshop"` 标记）；LR 恒写的
     2 点恒等主曲线折叠为空。局部滑杆刻度精确反演（曝光 /4→×4 二的幂无损；
     ×100 后 4 位小数吸附回 UI 网格）。round-trip 性质测试钉死：
     recipe→XMP→recipe 对写入器舍入安全值全等。`crs_f32` 迁至 xmp.rs
     （eval.rs `pub(crate) use` 转出，style.rs 路径不变）。
  2. **打开即恢复（gui.rs `read_saved_develop`）**：新开分支优先
     `<stem>.recipe.json`（无损：位图蒙版/color_gains/role 全回）、缺则 XMP
     反演；无效/中性边车恢复无物（`is_noop` 门）。undo 基线=恢复后配方，
     「重置」可回中性；状态栏 "ready — restored saved edits ({kind})…"。
  3. **保存改无损**：Save XMP (Ctrl+S) 同时写 recipe.json；反推 worker 对
     **任意**源持久化 recipe.json（RAW 另写 XMP 不变）——反推结果首次可
     关闭→重开完整还原。
  **响应性快赢（同批 `3124596`，源自 63-agent 对抗验证审计的 29 项确证）**：
  ① "● edited" 徽标每可见行每帧 2 次文件 stat → 按索引缓存
  （`edited_badge`，换目录/本 app 写边车时失效）；② 解码底图 LRU
  （`base_cache` 4 项，path+edge+mtime 键）——图库来回挑片二次打开跳过
  整幅 demosaic；③ 范围蒙版 overlay 的 masks-cleared 参考重建（UI 线程整幅
  显影，2560/4096 达 100-300ms）在指针按住期间挂起、松手后一帧内补建
  （几何蒙版不受影响）；④ 涂抹蒙版纹理改 `TextureHandle::set` 原地更新
  （原每笔一帧新建纹理）；⑤ `build_preview` `into_rgb8` 移动缓冲
  （原 to_rgb8 每 tick 深拷贝 ~3.3MB）；⑥ 目录扫描 `DirEntry::file_type()`
  免每文件二次 stat（符号链接回退 `Path::is_dir` 保持行为）。
  基线 **102 lib + 9 gui** 全绿（+5 xmp round-trip、+1 边车优先级、
  +1 LRU）、clippy(gui) 零警告。待用户真机验收：打开带 ● edited 的照片应
  直接回到保存的编辑；反推→关→重开应完整还原（含分区蒙版）。
  ~~引擎性能批次 #3-B backlog~~ → **已全部随 v0.11.0 落地**（见上一条；
  唯一形态调整：打开路径的"降采样后显影"以更安全的等价方案落地——中性
  tone-pass 短路 + 全管线并行化，预览像素不做线性/伽马域缩放置换）。
- **GUI 多语言 i18n：英文骨架 + 中文切换（2026-07-11，已随 v0.9.0 发布 → `ca6f73e`）**
  ——把原生 GUI ~430 条中英混排硬编码文案统一到零依赖、英文即键的翻译层。
  发布前 4 路对抗审计（密钥/范围/键覆盖/MaskRole）0 blocker，键覆盖审计报出
  3 条工具提示缺中文（WB 吸管 hover / 裁剪提示 / 镜头提示）——已补全并逐字节
  核对键在 gui.rs 调用点与 i18n.rs 目录两侧各出现一次（否则中文模式静默回退英文）。
  1. **新模块 `src/bin/i18n.rs`（`cad6c68`，557 行）**：`Lang{En,Zh}`（Copy+serde，
     默认 En）、`tr(lang,en)`（En 原样返回=骨架；Zh 查 `ZH_ENTRIES` 缺则回退 en）、
     `trf(lang,en,&[(name,val)])`（运行时 `{name}` 替换——`format!` 要编译期字面量，
     翻译串只能运行时插值）。`ZH_ENTRIES` = 唯一译文目录 = 语言版本控制单一来源。
     键必须与调用点英文字面量逐字节一致，否则 Zh 静默 miss（编译不报、测试也过）。
  2. **gui.rs 全量路由**：~430 条用户可见字面量过 `tr`/`trf`；每渲染函数顶
     `let lang = self.lang;`（Lang: Copy，不借 self，避开 worker 闭包借用冲突）。
     `Prefs.lang`（serde 容器级 default → 旧存档缺字段解码为 En，不重置其他偏好）、
     `AutoshopApp.lang`、save/restore 已接；设置区 Language 下拉切换下一帧即生效。
  3. **MaskRole 蒙版名解耦（recipe.rs/fit_zoned.rs）**：`enum MaskRole{#[default]
     Custom,ZoneSky,ZoneLand}` 挂 `LocalAdjustment.role`（`#[serde(default)]`），把
     分区蒙版身份从可翻译显示名剥离——名字可翻译而不破相等判断与 recipe.json 往返。
     engine-only，**不进 XMP**（xmp.rs 零引用，Bitmap 蒙版被写入器整体跳过，已验）。
     3 处 zoned 测试从 `m.name==` 迁到 `m.role==`。旧 recipe.json 缺 role 解码为
     Custom；新写 recipe 被更旧 build 读会因 `deny_unknown_fields` 报错（前向不兼容，
     app 内部数据、无 XMP 影响，可接受，同 color_gains 先例）。
  4. **Cargo.toml `autobins=false`**：两 `[[bin]]` 均显式声明，使 `src/bin/i18n.rs`
     （无 main）作 gui.rs 子模块而非独立二进制目标。
  基线 **97 lib + 7 gui** 全绿、clippy(gui) 零警告、双 exe 重建（gui 33451827 /
  cli 25974719）。范围仅原生 GUI——Web UI（index.html/serve.rs）、CLI（main.rs）未
  动。待用户 GUI 真机复测手感（英文默认 → 设置切中文 → 全 UI 中文；缺译回退英文）。
- **性能批次 #2-C：预览卡顿根治（2026-07-10，已随 v0.8.1 发布 → `ce69f27`）**——用户报
  "处理图片时会有些卡"。多代理只读剖析 + 无头基准定位两层根因，各根修：
  1. **色偏增益 LUT 化（`759c9ca`，render.rs）**：v0.8 分区蒙版的
     `color_gains` 在 apply_wb / apply_masks 里逐像素逐通道跑两次
     sRGB↔线性 `powf`——1280×853 生产型基准实测单天空蒙版 613ms/帧、
     天空+地景对 1208ms（同蒙版去掉色偏仅 53/92ms，证明幂运算占 ~90%）。
     根修：把精确线性光增益编成每通道 4096 项 LUT（复用色调阶段同款采样器），
     单蒙版降到 81ms、对降到 149ms，且 8-bit 预览**逐字节不变**（基准校验和
     不变；LUT 插值误差 <1.5e-5 < 1/255 量化，`colour_gain_lut_matches_the_
     exact_linear_light_formula` 钉死）。附 `preview_mask_perf_probe`
     (#[ignore] release-only 机器相对基准，带校验和防"跳步取胜")。
  2. **预览异步 latest-wins（`c1e8b8d`，gui.rs）**：display 原来同步跑在
     egui `update()` 里，整帧构建把 UI 冻住（2560/4096 100-300ms，带 v0.8
     色偏蒙版 0.6-1.2s）。改单后台 worker：`build_preview` 引擎显影+几何+
     单次 rgb8 转换（喂直方图/削波/缩略图）离开 UI 线程；完成回调丢弃
     (base,recipe) 已变的陈旧帧，快拖自动合并到 worker 吞吐、指针 60fps 不卡。
     Arc 共享 base 像素（派发 O(1) 非 50MB 深拷贝）、纹理 `TextureHandle::set`
     原地更新（不再每 tick 新建纹理管理项）、蒙版 overlay coverage-aware key
     （局部效果滑杆改"做什么"不改"作用范围"→不重建整帧覆盖栅格；纯几何蒙版
     不再跑第二次 masks-cleared 显影，仅范围蒙版保留）。顺带修复：蒙版"反转"
     复选框 `Response.changed()` 被丢弃（切换只改配方不重渲染）。无头测试
     `async_develop_discards_stale_frames_latest_wins` +
     `overlay_skips_rebuild_for_local_effect_sliders`（egui::Context::default，
     不 run_native）。基线 **97 lib + 7 gui**，clippy(gui) 零警告，双 exe 重建。
     待用户 GUI 真机复测手感（尤其 2560/4096 拖滑杆、带天空/地景双蒙版反推）。
- **反馈批次 #2-B：语义分区反推（2026-07-09/10 夜，随 v0.8.0 发布）**
  ——跨"区域性观感 vs 全局滑杆"表达力鸿沟的正路，全程 fail-first +
  真机对（_DSC9621 × reimagine-5）驱动迭代 4 轮渲染目视：
  1. **引擎局部 temp/tint（`d58ca60`，render.rs）**：LocalAdjustment 自 v1
     就带 Temp/Tint 且 XMP 会导出，但引擎从不渲染（GUI 蒙版滑杆拖了没反
     应的既有缺口）。`local_temp_to_kelvin`（相对 ±100 → mired 线性 ∓80
     围绕 5500K 锚，≈半张 CTO/CTB，render.rs）+ apply_masks 每蒙版一次
     `wb_gains`、线性光逐像素、WB→tone→sat 镜像全局次序。fail-first 红→
     绿 + 满帧蒙版≈全局 WB 等价性测试钉死映射与 tint 符号。
  2. **color_gains 重着色增益（`9c55e24`，recipe.rs/render.rs/fit_zoned.rs）**
     ——实测出的模型上限：调色板移植（蓝天→金天）要求线性 r/b ≈5.3×，而
     **任何** WB 参数化（扫满 2000–40000K 黑体）封顶 ≈1.9×、±100 饱和只
     ×2——Temp/Tint/Sat 物理上画不出重绘。新字段
     `LocalAdjustment.color_gains: Option<[f32;3]>`（线性光逐通道增益，
     0.05..8 钳制，中性收敛回 None；引擎专用——经典 ACR 无对应物，本来就
     只挂在 XMP 会跳过的 Bitmap 蒙版上），apply_masks 与 WB 增益乘法合成。
     可识别性论证：全局 cast 曲线必须重门槛是因为"哪里"未知；蒙版回答了
     "哪里"，区上逐通道增益就是可识别的——这正是表达力升级本身。
  3. **fit_zoned.rs 新模块（`9c55e24`+`a5173b2`+`09172f2`，~700 行）**：
     zone_moments（蒙版加权线性光一阶矩）→ fit_zone_dials（增益=want/2^EV
     精确闭式）→ `fit_recipe_zoned` 编排：全局 fit 先行 → `segment_file`
     ×2（源+目标各一次天空分割）→ **天空区 + 地景区**（同一栅格
     `inverted=true` 复用——第一轮真机渲染的教训：只修天空留下蓝晕带贴着
     金天空）→ 每区独立验收。分割/依赖/退化任何失败 → 优雅回退纯全局
     fit + rationale 注记，绝不报错。
  4. **分区验收哲学（`09172f2`，真机实测驱动）**：帧全局 look_err 会按构
     造否决正确的分区重绘（实测：天空区矩 0.507→0.016 落点几乎精确，帧
     全局却 0.1768→0.1792——生成式目标的天空占比 8% vs 23% 构图不同 +
     蓝→金迁移带质量被 worst-band 色相项读成伤害）。分区 do-no-harm 裁判
     = **区内矩误差**（zone_err ≤50% 原值）+ 帧全局仅作**有界漂移保险**
     （±0.02，实测漂移 +0.0024）。非对称占比回归测试钉死该几何。
  5. **区内色调 CDF 求解（`09172f2`）**：线性均值匹配后地景仍读起来暗很
     多（目标地景=日照台地+深峡谷阴影，亮像素统治线性均值、感知跟随分
     布）。区内加权 luma CDF → quantile 映射 → 复用 `fit::fit_tone_sliders`
     （同全局 stage-1 基底+幅度先验）解 6 个局部色调滑杆；**可识别性守卫**
     （实测）：近单值源区（平雾天空 IQR<0.05）上 quantile 映射退化（解出
     EV −0.70、区残差 0.016→0.108 倒退）→ 回退矩-EV、色调保持平。
  6. **接线（`b78daeb`）**：CLI `match --zoned`（蒙版落 GUI 约定
     out/<stem>.mask-sky.png）；GUI `zoned_fit` Pref（eframe 持久化，默认
     ON——有优雅回退）、Settings「反推」区开关、start_fit 分支 + 完成注记；
     XMP 诚实注记由构造完成（rationale 进 sidecar 注释 xmp.rs:350，Bitmap
     蒙版被跳过 xmp.rs:79）。
  7. **真机 v4 验收（无头 CLI + 渲染目视 + 数值）**：双分区 attach（天空
     0.507→0.016、地景 0.151→0.006）；天空奶金 [0.69 0.60 0.48]（目标上
     天空 [0.63 0.56 0.50]）、地景暖红棕有结构、无蓝晕、无 re-hue；地景
     亮度较目标仍差 ~0.1 sRGB——目标重打光了构图（诚实残余，rationale 有
     注记），且蒙版滑杆现已实时渲染，用户面板一拖即补。测试基线
     **96 lib + 5 gui**，clippy(gui) 零警告。**待用户 GUI 真机验收**。
- **反馈批次 #2-A：反推统计加固（2026-07-09 深夜，`7471d35`，本地未推送）**
  ——用户 v0.7.0 真机复测报"效果还是不好"（_DSC9621 × reimagine-5：全图刷
  成高饱和橙、天空由雾蓝变橙桃）。实测定位三个根因并全部 fail-first 修复
  （用户选定方向 C = 先统计加固后分区反推）：
  1. **旋转预算门（第三道 cast 门，fit.rs）**：目标是"调色板移植"级的全暖
     AI 渲染时，蓝通道 CDF 匹配把雾蓝天空整区 re-hue ~170° 进目标原生橙——
     外来色否决按设计不拦（落点是目标原生色相）、聚合门被全图通道均值改善
     抬过（实测 ratio 0.25）。新门做像素对齐旋转普查：两端都可见着色
     （chroma≥0.04）且色相移动 ≥75° 的像素占画面 ≥5% → 拒。阈值全部实测
     标定（雾霾修正 75° 处 ≈0、紫峡谷 112°、金天空 ~170°）；书面代价：重
     色偏校正若需 >75° 旋转也会被拒（保守失误可在显影面板补救，区域 re-hue
     不可救）。
  2. **色调证据对称化（`tone_cdf_pair`）**：中性门是"两侧同一人口"的可识别
     性假设——旧代码两侧独立决定，目标把淡色区 re-hue 出中性集时（雾蓝天空
     中性、金天空不中性），一侧中性 CDF 对一侧全像素 CDF，色调解算整体畸变
     （真机对里 Shadows −49 的诡异组合即此来源）。现成对判定：任一侧样本不
     足或中性份额比 >1.75× → 双侧一起回退全像素 CDF。
  3. **do-no-harm 终检**：饱和度是唯一按启发式（均值彩度追赶）拟合的旋钮且
     中途不可评判（正确的饱和会先放大潜在色偏，曲线阶段再清除——阶段局部验
     证门试过，砍死雾霾回归的 sat 被否决）；管线终点若整体 look_err 比不动
     还差则折半 sat 并重拟曲线。其诱因已被 #2 根治，现无 fixture 可达，作
     为保险留存（代码注释明示）。
  4. **诚实面**：rationale 新增三类注记（残差仍远 >0.12 时建议直接用 AI 变
     体或分区编辑 / sat 顶格 ±60 / 曲线因 re-hue 风险被扣），confidence 随
     诚实残差自然下降（真机对 0.73→0.25）。
  测试 82→85 lib：金天空策略回归（修前 sky 49°、Δ164° 失败）、**真机几何
  布线测试**（雾霾源×全暖目标，旋转门是唯一拒绝者——变异验证：摘门即败；
  峡谷合成对上 ratio 门冗余拒绝、看不见该变异）、旋转份额 pin（0.1× 余量
  同时从下方钉住 ROT_DEG）。真机对无头复验：曲线全扣、天空保持蓝、err
  0.275→0.177 诚实上报。附注：多代理对抗审查揪出 9 项实证问题全部处置
  （含两个变异验证的"未承重"洞、rationale 误导措辞、文档漂移、测试复制生
  产代码——审查代理曾误留 eprintln/stash 污染工作树，已恢复并全量复验）。
- **反馈批次 #1 → v0.7.0 已发布**（2026-07-09，tag `v0.7.0` → `7c36ee3`，
  双 exe 资产字节核对 33286921/25914296，标记 Latest）——用户真机报障
  "反推紫天空 + 扁平"（峡谷照截图对）+ 四项指令（解决问题/加去雾、修 bug、
  代码库 debug+优化、优化 UI）：
  1. **反推紫天空根治（`7b6a64c`，fit.rs）**：cast 曲线的聚合验收门对
     **跨带色相灾难结构性失明**（天空被染紫后质量落进目标为空的紫/品红带、
     又流出蓝带——双侧带权门把两边都跳过，色相项什么也看不见）。根治 =
     第二道**外来色否决**（`cast_paints_foreign_hues`）：有/无曲线两次渲染
     像素级对齐，曲线把 ≥5% 画面涂到离目标一切色相 ≥45°(1.5 ACR 带) 的
     色相上即拒。判别量是**色相距离**（实测：峡谷紫距目标 60°+、雾霾修正
     残差仅 5-40°——任何单一粒度的"色族成员"规则都会误判一边：±15° 细窗
     把雾霾修正判 15% 伤、整带份额把其橙黄裙边判幻影黄）。fail-first 复现
     用例 `warm_rock_cast_must_not_violet_the_pale_sky`（红偏移暖化——乘性
     暖化会被保色相的饱和度阶段吸收、根本不触发 cast 曲线）+ 判据 pin 测试
     `foreign_hue_veto_separates_haze_from_canyon`（峡谷 2× 余量触发、雾霾
     0.000 不触发）。
  2. **去雾引擎落地（`66062be`，render.rs）**：`dehaze` 字段原是**空壳**
     ——GUI 滑杆在、XMP 导 `crs:Dehaze`，但渲染管线从不读它。现为
     apply_develop 阶段 0b（暗角后、色调 LUT 前）：线性光散射反演
     `I=J·t+A(1−t)`，逐像素 min 通道当雾密度（**非**空间暗通道滤波——
     O(N) 且统计上 CDF 可辨识）、airlight=min 通道线性 P99 直方图（跨分辨率
     稳定）、单仿射保通道序（无品红/青反转）、v=A 定点保亮天、负值=凸组合
     加雾不裁剪。5 个测试（物理构造的雾夹具 t=0.55/A=0.9）。**反推刻意不加
     dehaze 阶段**：色调 CDF+饱和度之后其唯一残差特征（亮度-彩度联合轮廓）
     对生成式目标内容混淆，与已删的 per-band HSL 同类。
  3. **两个"永久卡死 busy"根因修复（`9b60a62`）**：① 4 个 AI ureq 调用
     **零超时**（默认 agent 无读取期限——桥挂/代理停摆 = 工作线程永久阻塞、
     GUI 全锁）→ `advisor::post_with_timeout`（connect 10s + 按延迟等级
     propose 120s / style 90s / verify 60s / images-edits 300s，
     `AUTOSHOP_HTTP_TIMEOUT_SECS` 全局覆盖）；② 工作线程**无 catch_unwind**
     （rawler/image 对坏文件 panic = 终结 Msg 永不到达）→
     `AutoshopApp::spawn_worker` 唯一 spawn 收口点，panic 合成该点位的失败
     Msg，15 个站点全部收编（fetch_models 的手写 RAII guard 被统一替代）。
     附 blur_plane 零维守卫（`Ord::clamp(0,w-1)` w=0 会 panic 的潜在类）。
  4. **滑杆流畅度 + chrome 打磨 + 快捷键速查（`24ed6a3`）**：拖动中显影
     **自适应合并**（1.5× 实测显影耗时，33-500ms 夹取；4096px 预览从每帧
     100-300ms 同步显影的卡顿降到 ~40% 占空比，松手帧立即显影不失真）+
     变体缩略图中拖跳过 + 位图蒙版 (path,mtime) 键进程级解码缓存（分割重跑
     覆写同文件，只按 path 会钉死旧蒙版）；画廊蓝/金双强调色统一为金 PILL
     系（**画布上工具覆盖层刻意留蓝**——金手柄在暖片上隐形，规则记录在
     常量处）；设置窗加滚动（保存键曾可能够不着）、状态栏截断+悬停全文、
     Export/Download/Save XMP/AI Analyze/Reset/Style/Fit 补 tooltip、面板
     标题双语化；**F1 / ? / ⌨ 快捷键速查表**（候选池项交付；O 蒙版覆盖层
     此前无任何可见控件）。
  基线 82 lib + 5 gui 测试、clippy(gui) 零警告、双 release 构建绿；无弹窗
  纪律全程遵守——真机验收（滑杆手感/紫天空实照重跑/去雾观感）待用户。
- **图像角色 OAuth 模式 / codex 桥（2026-07-09，`c389df6`，v0.6.0 发布）**：
  图像角色（vision 提配方 + 生成式 fill/heal/reimagine）新增 `image_provider`
  开关——**OAuth（本地 Codex 桥）** | **API（真 OpenAI key）**，与分析角色的
  OAuth|API **对称**。OAuth 模式经 CLIProxyAPI（`127.0.0.1:8317`，持有用户的
  ChatGPT 订阅令牌，上游 `chatgpt.com/backend-api`）走订阅出图，**无需 OpenAI
  key**；选 OAuth 自动填桥地址 `http://127.0.0.1:8317/v1`、API-Key 标签改
  「Gate token」、拉取模型按钮变可达性测试、image-gen 回退模型顺序按模式切换。
  纯 UI + 一个 config 字段（`config.rs` `image_provider`，缺省 `"api"` 保持旧
  行为；`image_is_oauth()` 判定；`gui.rs` `SettingsForm.image_provider_oauth`
  + 幂等自动填地址），**引擎零改动**——两种模式都落到同一 OpenAI 兼容 HTTP 路径。
  已知硬上限：订阅出图路径经 codex 内建 `image_gen` 工具，输出面积锁 ~1.57 MP
  （honors 宽高比、免费路不可提；全分辨率需真 OpenAI key 的 `images.request`
  scope）；遮罩编辑语义非像素保真但 `composite_region` 天然免疫。ToS 灰区、
  测试烧订阅额度——详见持久记忆 `autoshop-codex-bridge`。真机点击链未走（无弹
  窗纪律，靠编译 + 75 lib/5 gui 测试 + 源码走查验证）。
- **变体/版本条重构（2026-07-08，用户报障"AI 生图后再调整又变回去"+
  "图片版本没有可选的"，随 v0.6.0 发布）**：把"单一工作图 (src_path,
  base_preview, recipe)"模型换成**变体条**（Lightroom 虚拟副本 / Capture
  One 变体，**非合成图层**——它们不叠加，是同照片的平行版本）。一个变体 =
  (底片来源, 配方)：**原片**（底片=RAW，你的显影）/ **AI 生成**（底片=生成
  PNG 像素，观感烘焙其中）/ **反推**（底片=同一 RAW 中性，观感在配方 → 可
  编辑/导 XMP/出全分辨率）。底部缩略图条点击**无损切换**（各记各的底片+配方）；
  生成→自动新建「AI 生成」变体并切过去（编辑的就是生成图当底片，**不再变
  回去**）；反推→自动新建「反推」变体；fill/heal/clone→就地更新当前变体像素。
  **彻底移除** master/master_restyled/open_note/continue_from_master/「以此
  母版继续修图」整套绕过——`2fc9092` 的补丁被此结构性正解取代（各变体天然
  隔离，不可能二次烹饪）。关键正确性修正：反推的拟合底改用 `source_preview`
  （生成后 base_preview 已是生成图，拿它当底会把生成图拟合到自身≈中性）。
  UX：统一视觉主题（`install_theme`——PILL 金强调/圆角/间距/标题字号）。
  **对抗审查两轮 + 一次同步终审**（Workflow 多代理，各发现独立证伪）：
  第一轮 6 项确认全修（生成变体上 fill/heal/clone/导出/XMP 曾误用 src_path →
  统一 `active_source_path()` 变体感知像素源；`delete_variant` 补 busy 卫；
  retouch 结果按 `preview_edge` 烘焙不再降清晰度并重建 mask_paint；失败开图
  清工作态防 src_path↔变体错位；生成 origin 文件系统探测唯一命名防
  delete-then-reimagine 别名）；第二轮 2 项确认全修（就地修补后 repoint
  `variant.origin` 使导出/反推/续修跟随修补像素 = WYSIWYG；Download 建议名
  跟随 active_source_path）；终审 CLEAN。gui.rs ≈ +600/−200。测试 75 lib +
  5 gui、clippy 0、release、最小化烟雾均绿。已知边界（非本次引入）：修补过的
  原片变体导出 TIFF 含修补像素、但 XMP 是参数式无法承载像素修补，二者会分歧。
- **v0.5.2 已发布**（tag `v0.5.2` → `a57be95`，双 exe 资产字节核对
  33174745/25899129）：UX 批次5（`d987c5b` 顶栏换行不裁按钮+最小窗口/
  蒙版图上手柄编辑/放大后拖拽即平移）+ 反推配方根治（`6de045d` 下条明细）。
  注意用户指令（2026-07-07）：**调试时不弹窗**——引擎级改动跳过 GUI 启动
  烟雾，动了 gui.rs 才启动且最小化。
- **反推配方修复（2026-07-07，用户真机报障"反推 XMP 之后很奇怪"）**：
  紫天空/橄榄岩/滑杆顶死（Contrast −97、Shadows −100、红橙 hue +45）在
  用户的 _DSC9621 真对上逐位复现并分阶段渲染定位，三个根因全部根治
  （fit.rs）：①色调求解病态——近共线基底+名义岭（1e-4）让"巨大对冲"组合
  靠 ε 获胜，现 `TONE_PRIOR=0.02` 同时做岭和曝光扫描的选型惩罚；
  ②按带 HSL 拟合对非像素对齐的生成式目标**统计上不可辨识**（带心色相差
  把内容差异误读为旋转，13° 门限内"可信"证据整带旋转即成灾）——整级删除，
  按带意图归风格提示词路径（与局部蒙版同理）；③通道色偏曲线改**验证式
  接受**——均匀色偏（雾霾）一通道一映射是精确模型、内容差异则是错误模型，
  以带色相项的 look_err 降到 ≤0.85× 才保留（`CAST_ACCEPT_RATIO`）；
  look_err 本身补上**最差带**色相项（加权平均会让小面积的天空灾难隐身，
  实测混过验收门）。新回归 `hazy_to_clean_fit_stays_sane` 钉死：无退化
  滑杆、误差严格改善、拟合后每个有像素带的色相偏差 <15°。真对最终渲染
  蓝天+自然暖岩，置信度诚实降 0.80→0.43。测试基线 75 lib + 5 gui。
- **v0.5.1 已发布**（tag `v0.5.1` → `b92d4f3`，双 exe 资产字节核对
  33190058/25904350）：UX 四批 + debug 清扫整批（下条明细）。
- **v0.5.0 tag 之后的提交（2026-07-07 已 push，随 v0.5.1 发布）**：
  `763a2bc` UX批次1（蒙版覆盖叠加/削波警告/Esc）→ `eb6a098` 叠加参考缓存
  → `51c151d` UX批次2（hover 预览/直方图三角灯/批量进度条）→ `be60c52`
  UX批次3（蒙版⬆⬇排序/光标语言/缓存key收窄）→ `55e7e07` **debug 清扫**：
  ①方向统一——rawler ARW 内嵌预览不带 EXIF 转正（crate 源码实证），旧管线
  在 develop **之后**才 oriented() ⇒ 竖拍 RAW 的 crop/straighten 会错轴；
  现两侧都在最前端转正（引擎 `orient_f32` 复用同一 `oriented`，decode 端
  `preview_only`/`decode_raw` 同函数转正），Normal 方向逐位不变，回归
  测试+真 ARW 61MP 全流程实测；②hover_mask 改帧作用域（折叠面板/换图
  不再粘滞）→ `a494156` ROADMAP 交接刷新 → `4f16a8c` UX批次4（削波
  三角按通道显色/蒙版真拖拽排序/裁剪柄方向光标——候选清单清零）。
- **v0.5.0 已发布**（tag `v0.5.0` → `3ab41b6`，双 exe 资产字节核对）：
  三大项整批——C2 手动畸变 / D2 P3+AdobeRGB 真 gamut 导出 / A② AI
  主体天空分割（位图 mask 通路 + python sidecar，用户真机实测）。
- **v0.4.0 已发布**（tag `v0.4.0` → `e175bf8`）：范围蒙版 / 双轨续接 /
  导出管线 / 高分预览 / 暗角补偿 / sRGB ICC / 版本快照——A-G 整批。
- **~~C2 手动畸变校正~~ ✅ 完成（2026-07-06 深夜，见 §C，提交 b623e5a）**
  ——坐标映射整体设计（original→corrected→view 三空间合约）+ 引擎径向
  重映射 + GUI 全调用点接入 + XMP，67 lib + 4 gui 测试。
- **~~D2 P3/AdobeRGB 输出~~ ✅ 完成（2026-07-06 深夜，见 §D）**——真
  gamut 变换（色度推矩阵 + 双 TRC）+ CC0 profile 双件 + GUI 色彩空间
  下拉 + Prefs，69 lib + 4 gui 测试。
- **~~A② AI 主体/天空分割~~ ✅ 完成（2026-07-07 凌晨，见 §A）**——
  引擎位图 mask 通路（MaskGeometry::Bitmap + 双线性采样 + XMP 跳过）+
  `python/segment.py` sidecar（subject=rembg U²-Net / sky=SegFormer
  ADE20K，实测均通）+ GUI 两键入口，72 lib + 4 gui 测试。
- **三大项至此全部触底。** 剩余工作 = 各节「未做/已知边界」小项（去紫边、
  Upright、lensfun、位图 overlay 半透明显示、tile 金字塔、水印等）+
  真机验收清单；无未开工的大工程。
- v0.3.0 → `fa9add8`，v0.2.0 → `1bc57ff`。
- **有序批次 ①-⑤ 全部完成**（详见各节 ✅ 小节，含实现锚点与已知近似）：
  ①曲线编辑器 ②批量复制/粘贴 ③WB 吸管（含 WB 预览前置重构）
  ④拉直（引擎真旋转+自动内接裁剪）⑤仿制图章（clone_raw 像素通路）。
- **差距批次 A① 亮度/颜色范围蒙版已完成**（见 §A ✅ 小节：recipe/render/
  xmp/gui/advisor 五层，60 lib + 4 gui 测试）。A②（主体/天空 AI 分割）
  未做——前置是引擎位图 mask 通路。
- **差距批次 B 双轨打通已完成**（见 §B ✅ 小节：母版路径入 GUI 态 +
  「⤴ 以此母版继续修图」保留配方续接）。
- **差距批次 F 导出管线已完成**（见 §F ✅ 小节：ExportOpts 长边/锐化/质量 +
  批量渲染 worker，61 lib 测试）。
- **差距批次 E 高分预览已完成**（见 §E ✅ 小节：1280/2560/4096 预览分辨率
  下拉，切换保配方重解码）。
- **差距批次 C 两片全部完成**（见 §C ✅ 小节）：暗角补偿（线性光域径向
  增益 + GUI 镜头校正区 + XMP VignetteAmount/Midpoint）+ C2 手动畸变校正
  （三空间坐标合约 + 引擎径向重映射 + GUI 映射链全接入 + XMP
  LensManualDistortionAmount，67 lib 测试）。
- **差距批次 D 第一步 导出嵌 sRGB ICC 已完成**（见 §D ◐ 小节：三格式
  显式编码器 + CC0 profile 入库，64 lib 测试）。
- **差距批次 G 版本快照已完成**（见 §G ✅ 小节：`<stem>.v<N>.recipe.json`
  编号快照 + 版本区存/载 UI）。
- 更早已上线：反推配方（`fit.rs` + CLI `match`）、gpt-image-2 弹性高分辨率
  （≤8.3MP + 400 回退）、风格提示词提取、GUI 生产化（直方图/toast/快捷键/
  拖拽/持久化/折叠分组/双击归零）。
- 待用户真机验收（v0.3.0 起累计）：曲线拖拽/吸管/图章/拉直/范围蒙版
  手感；「以此母版继续修图」链路（修补→动滑杆→再修补→导出）；导出长边/
  锐化/质量 + 批量渲染选中；预览 2560/4096 的滑杆延迟是否可接受；暗角
  补偿手感；版本快照存/载；导出 ICC 在广色域屏与真 LR 的显示；范围蒙版
  XMP 与 VignetteMidpoint 在真 Lightroom 打开的效果；持久化"正常关闭→
  重启恢复"；高分辨率生成与风格提示词的真实 API 行为（付费调用，有 400
  回退兜底）。**v0.5.0+UX 阶段新增待验**：AI 选主体/选天空按钮真机手感
  （GUI 内点击链路，sidecar 命令行已实测）；畸变滑杆与真 LR 同数值强度
  对比；P3/AdobeRGB 文件在广色域屏与印刷流程观感；蒙版覆盖叠加透明度
  （255,40,40 α≤140/255）与 hover 预览响应；削波三角灯灵敏度（任一像素
  即亮）与按通道显色可读性；批量进度条；蒙版拖拽排序手感（浮影/插入线，
  ⬆⬇ 按钮保留）；裁剪柄方向光标；**竖拍 ARW 全流程**（方向统一后
  显示/蒙版/裁剪/拉直应全部正确——修复靠单测+横拍实测，竖拍样张未过）。
  **v0.5.2 新增待验**：窄窗口下顶栏折行观感；蒙版手柄命中半径（12px）
  与拖拽手感；放大平移 vs Ctrl 框选切换顺手度；**反推配方在 GUI 内重跑**
  （引擎路径已在用户 _DSC9621 真对上复现→修复→复验，GUI 点击链路同函数）。
  **变体条重构新增待验（随 v0.6.0 发布）**：① 生成出片→底部出现「AI 生成」变体
  并自动切过去、微调滑杆不再变回原图；② 反推→出现「反推」变体（滑杆可编辑、
  RAW 写 XMP）；③ 缩略图条点击在 原片/AI 生成/反推 间无损来回切换；④ 停在
  「AI 生成」变体上 Export/Download **导出的是生成图像素**（非原片中性）、
  Save XMP 提示先反推；⑤ 在生成变体上 fill/heal/clone 修补的是生成图、且导出
  跟随修补（WYSIWYG）；⑥ × 删除非原片变体；⑦ 生成两次得两个独立「AI 生成」
  变体互不覆盖；⑧ 统一主题观感。真机点击全链未走（状态机经编译+75/5 测试+
  两轮多代理对抗审查+同步终审 CLEAN）。**#2-B 分区反推新增待验（GUI 链路）**：
  ① 新 build 里对 _DSC9621 × reimagine-5 重跑反推（Settings「反推」区开关
  默认 ON）——应出现「反推·天空」「反推·地景」两个蒙版、天空转奶金、无蓝晕
  无紫（CLI 无头链路 v4 已目视+数值验收，GUI 点击链路同函数）；② 地景若嫌
  暗，在蒙版面板拖「反推·地景」的 Exposure——蒙版滑杆现已实时渲染（顺带验
  Temp/Tint 从"仅 XMP"组移入实时区后拖动即时生效）；③ 首跑天空分割会下载
  segformer-b0（~14MB，看状态栏提示）；python 依赖缺失时应静默回退纯全局
  反推且 rationale 有说明。

## 关键架构事实（新会话必读）

- 所有图上交互经 `ViewXform`（屏幕↔全幅归一化，gui.rs）；工具互斥分发在
  `after_view`（crop > placing > wb_pick > range_pick > clone > paint >
  box-select）。
- **EXIF 方向在链条最前端**（55e7e07 起）：引擎 `orient_f32` 在 develop
  之前转正 f32 缓冲，decode 端 `preview_only`/`decode_raw` 用同一
  `render::oriented`（pub(crate)）转正内嵌预览——GUI 显示帧 == 引擎
  original 帧，任何 RAW 方向下蒙版/裁剪/拉直坐标一致。rawler 的 ARW
  内嵌预览本身**不带**转正（crate 源码实证）。
- `develop_preview`（render.rs）跑 `apply_recipe_wb` + `apply_develop`；
  **不应用裁剪**（GUI 用 uv 窗显示、导出端真裁）。**几何链**由 GUI `redevelop`
  在 develop_preview 之后依次调引擎 `apply_lens_distortion`（C2 畸变）→
  `rotate_straighten`（拉直）完成（导出路径同函数、同顺序）。
- **坐标空间约定（④起，C2 扩展）**：original →（畸变校正）→ corrected →
  （旋转+内接裁剪）→ view；`recipe.crop` 存 view 空间；masks/画笔/吸管/
  region 存 original 空间——gui.rs `view_norm_to_orig / orig_norm_to_view /
  geom_to_view`（三者带 `dist` 参数，来源 `geom_ctx`）在数据边界换算，共用
  引擎 `inscribed_dims / distort_norm / undistort_norm`，全零恒等。完整
  合约见 render.rs "Manual lens distortion" 注释块。
- tone 模型单一事实来源：`render::TONE_KNOTS_X / tone_slider_basis /
  tone_exposure_curve`（pub(crate)，fit.rs 逆着它解）；曲线采样单一事实来源
  `render::curve_lut`（pub，GUI 曲线编辑器直接画它）。
- `recipe.masks` 是 AI 与手动共用的同一列表；引擎 `apply_masks` 实时渲染
  **WB(temp/tint)+color_gains → tone → saturation → NR**（#2-B 起；WB 镜像
  全局 `wb_gains` 模型、mired 映射 `local_temp_to_kelvin`；`color_gains`
  是分区反推的重着色增益，引擎专用），clarity/dehaze/texture 仍仅进 XMP
  （GUI 已如实分组：Temp/Tint 移入实时区）。
- 分区反推 `fit_zoned.rs`：`fit_recipe_zoned`（CLI `match --zoned` /
  GUI `zoned_fit` Pref）= 全局 fit → 天空分割×2 → 天空+地景（同栅格反相）
  双分区 → 每区 zone_err 矩裁判（帧全局 look_err 只作 ±0.02 漂移保险——
  帧级指标会否决正确分区重绘，实测记录在 ZONE_ACCEPT_RATIO 注释）＋区内
  luma-CDF 色调求解（源区 IQR<0.05 退化守卫）。任何失败优雅回退全局 fit。
- 照片库 `D:/Photography` 只读；输出一律 `./out`（`pipeline::guard_readonly`，
  项目自身 `./out` 永远可写）。

## ① 色调曲线交互编辑器（✅ 已完成）

- 数据已通：`recipe.tone_curve/red_curve/green_curve/blue_curve:
  Vec<CurvePoint{input,output: u8}>`（recipe.rs）；引擎组合方式——master 曲线
  在滑杆样条**之后**复合（`build_tone_lut` 末尾 `sample_lut(&curve, hermite_eval…)`），
  RGB 曲线在 master 之后（`apply_rgb_curves`）；分段线性 `interp`。
  XMP：`ToneCurvePV2012(+Red/Green/Blue)`（xmp.rs `curve_elem`）。
- GUI 设计：develop_panel 新 CollapsingHeader「曲线 · Curves」；通道选择
  （主/R/G/B）；自绘 widget：`allocate_exact_size(~边长 220)` + painter——
  网格、直方图背景（有 `self.histogram`）、曲线线条（按引擎同款 `interp`
  采样保真）、控制点拖拽（命中半径 ~8px）、空白处点击加点、拖出框外删点
  （LR 手势）、input 保持严格递增去重。改动 → `clamp()+dirty`。
- 无引擎改动。测试：曲线点排序/去重的纯函数可单测。

## ② 批量：配方复制 / 粘贴 / 同步（✅ 已完成）

- GUI：gallery 支持 Ctrl+点击多选（现 `selected: Option<usize>` 单选，加
  `HashSet<usize>`）；按钮「复制配方」/「粘贴到选中(N)」。
- 粘贴 = 对每张写 `write_recipe` + `write_xmp`（./out，RAW 才有 XMP），
  可选跳过 crop/straighten（LR 同步对话框的简化版：一个 checkbox）。
  worker 线程跑批 + 状态/toast 汇报；沿用 `Msg` 通道模式。
- 不渲染成品（可选 flag 后续加）；库只读不变。

## ③ WB 吸管（✅ 已完成，含前置）

- **前置已做**：新共享阶段 `apply_recipe_wb`（render.rs，apply_wb 旁）接入
  develop_preview / render_to_image / render_baked_to_image 三条路径；
  `temperature_k.is_some() || tint != 0` 即生效（修复了 tint 单独无效的旧坑）。
- 吸管已做：`render::solve_wb_from_neutral`（对数网格扫 K 使 r≈b，绿残差
  解析出 tint，与 `wb_gains` 同一正向模型）；GUI 色调区「💧 吸管」按钮 +
  图上点击取 5×5 均值（取 base_preview 的 pre-develop 像素）。
  单测：合成偏色像素 → 反解中和（<2% 残差）+ 预览 WB 生效性。

## ④ 拉直（✅ 已完成）

- 引擎：`render::rotate_straighten`（顺时针、双线性、16-bit）+ 公开的
  `render::inscribed_dims`（闭式最大内接矩形），在两条导出路径的用户裁剪
  **之前**、orientation 之后应用；GUI `redevelop` 用同一函数旋转预览。
- 坐标空间约定（重要）：`recipe.crop` 存**拉直后**空间（导出旋转后裁剪，
  裁剪工具无需映射）；masks/画笔/吸管/region 存**原始**空间——gui.rs 的
  `view_norm_to_orig / orig_norm_to_view / geom_to_view` 在数据边界换算
  （共用引擎 inscribed_dims，0° 恒等，roundtrip 有单测）。
- 已知近似（待真 LR 验证）：angle≠0 且带 crop 时 XMP 的 CropLeft…/CropAngle
  组合语义与我们的"先转后裁"是否逐像素一致未对照过真实 ACR 边车。

## ⑤ 仿制图章（✅ 已完成）

- 引擎：`HealSpot.clone_raw`（跳过 heal 的边界色调匹配 = 原样搬运 + 羽化）
  + `retouch::clone_stamp(src, mask, source_norm, full_res, out)`——涂抹 blob
  → spots，每个 spot 的供体偏移 = 源点 − blob 中心（PS 非对齐取样）。
- GUI：Retouch「仿制图章」节——进入图章模式，Alt+点击取源（十字标记，
  存原始帧坐标），共用画笔涂目标，「⎘ 克隆已涂区域」worker → ./out
  像素母版（同 heal，非 XMP）。单测锁定 clone（原样）vs heal（色调匹配）
  的语义差异。
- 已知近似：拉直角≠0 时画笔 overlay 纹理按原始帧直贴（落点计算正确，
  显示未旋转）——heal/clone/fill 共同的显示级问题，engine 结果不受影响。

## 与 Photoshop 的核心差距（2026-07-06 调查 · ①-⑤ 完成后）

> 定位前提：目标是"日常出片替代"（LR/ACR + PS 修图子集），不是 PS 的
> 设计/合成全集。按对日常出片的影响排序；「现状」均为当日代码实测。

### A. 智能选区 / 范围蒙版（① ✅ 2026-07-06 · ② ✅ 2026-07-07）
- PS/LR：Select Subject / Sky、亮度/颜色范围蒙版。
- **① 亮度/颜色范围蒙版 ✅**：五层打通，权重 = 几何 × 范围（相交）。
  - recipe.rs：`RangeMask` 枚举（Luminance 4 数梯形 = ACR LumRange 原样；
    Color = 参考色 rgb + amount 容差 + px/py 取样点）+
    `LocalAdjustment.range: Option<RangeMask>`（serde default，旧 JSON 兼容）；
    clamp 强制梯形非降序。
  - render.rs：`range_weight`（亮度=梯形 ramp，退化边=阶跃；颜色=亮度不变
    色度距离，除以各自 luma 后欧氏距离，d_max = 0.15+0.9·amount）；
    apply_masks tone + NR 双 pass 相乘接入。
  - xmp.rs：`range_mask_xml` 第二组件 `Mask/RangeMask`，相交编码
    `BlendMode=1 + Inverted=true + Value=0`（从用户自己的 LR 边车
    `_DSC9245.xmp`/`_DSC9303.xmp` 解码验证的代数）。
  - gui.rs：选中 mask 面板「范围蒙版」下拉（无/亮度/颜色）；亮度=下限/上限/
    羽化三滑杆（GUI 对称羽化 ↔ recipe 4 数梯形）；颜色=色块 + 🎯 取样
    （`handle_range_pick`：pre-mask develop 的 5×5 均值，与引擎评估像素
    一致）+ 容差滑杆；`range_picking` 入工具互斥。
  - advisor：openai.rs 结构化 schema 加 `range`（anyOf 双变体 + null）+
    prompt 用法指引。
  - 已知近似：(a) 范围权重按"全局显影后、蒙版逐个叠加时"的像素评估——
    多 mask 叠加时后面的 range 看到前面 mask 的输出（LR 是固定参考图；
    全分辨率快照内存不可行，已注释）；(b) 颜色 PointModels 第 4-6 数
    按"取样点坐标+保留位"假设写出，未与真 ACR 对照语义；(c) 真 LR 打开
    效果待用户验收。
- **② 主体/天空 AI 分割 ✅（2026-07-07 凌晨）**：位图 mask 通路 + python
  sidecar 两层全通。
  - **位图通路**：recipe.rs `MaskGeometry::Bitmap { path }`（`kind`-tag 序列化，
    JSON 往返测试）；render.rs `load_mask_bitmap`（每 mask 每次 develop 解码
    一次，绝不进像素循环；缺文件=惰性 + stderr 警告）+ `sample_gray_norm`
    （归一坐标双线性 → 1280 mask 驱动 61MP 导出）+ `mask_weight` 第三臂，
    tone/NR 双 pass 共享；xmp.rs 位图 mask 跳过（经典 ACR XMP 无法表达；
    全位图时不发空壳块——参数 mask 照常写出，§B 式定位取舍）；GUI 列表
    「位图」标签、overlay 徽标（不假装形状）、重画按钮对位图隐藏。
  - **sidecar**（`python/segment.py` + `src/segment.rs` 桥，循 denoise.py
    模式；config `segment_script` / `AUTOSHOP_SEGMENT_SCRIPT`）：
    `--target subject` = rembg U²-Net 显著主体软 alpha（`pip install rembg`，
    模型首跑自动下载 ~/.u2net，176MB）；`--target sky` = SegFormer-B0
    ADE20K 天空类概率（transformers，~14MB 自动下载；sky 类号从模型
    id2label 解析、不硬编码）。缺依赖时 exit 2 + 打印确切 pip 命令。
  - **GUI**：局部调整区「🤖 AI 选主体」「☁ AI 选天空」→ worker 喂
    ORIGINAL 帧预览 → `./out/<stem>.mask-<target>.png`（同 target 重跑
    覆盖同文件）→ 推入 Bitmap mask 并选中，undo 一步回退；软 alpha 即
    天然羽化。
  - **实测（2026-07-07，用户环境 Python313）**：天空 = Lundy 真照片
    天侧均值 254/地侧 0；主体 = 合成主体中心 255/背景 0/覆盖 18.7%
    （与真实面积一致）；纯风景无主体时主体 mask 近空属模型正常行为。
    rembg 需装进 `python` 对应环境（用户机上 `pip`≠`python -m pip`，
    后者才对）。
  - 已知边界：mask 位图不进 XMP（LR 侧丢 AI 选区）；位图 overlay 暂为
    徽标而非半透明叠加显示；分割跑在预览分辨率（对羽化选区足够）。

### B. 像素母版 ↔ 参数配方双轨打通（✅ 已完成 2026-07-06）
- 现状（旧）：fill/heal/clone 输出 ./out 像素母版，仅在 After 显示一次；
  滑杆一动即 redevelop 回配方渲染，母版脱链。
- **已实现**（gui.rs）：`Msg::Retouched`（`RetouchDone` 别名）四条像素路径
  （fill/heal/clone/reimagine）都带回母版路径 → `self.master`；Retouch 面板
  顶部「⤴ 以此母版继续修图」→ `continue_from_master`：一次性 `keep_recipe`
  标志让下一个 `Msg::Opened` **保留当前配方**（母版是同帧中性显影+修补像素，
  滑杆/蒙版/裁剪/拉直 1:1 适用），src_path 重定向到母版 → 后续修图/导出
  都基于它。undo 历史在新 base 上重开；master 随换图清空；打开失败也会
  消费掉 keep_recipe（不泄漏到无关的下一次打开）。
- 边界：XMP 仍只随 RAW 源写出（母版是 PNG，只写 recipe json）——参数轨
  的 Lightroom 出口停在原 RAW 一侧，属定位内取舍。GUI 态逻辑无单测
  （egui app 态），入真机验收列表。

### C. 镜头/几何校正（✅ 暗角 + C2 手动畸变均完成 2026-07-06）
- **暗角补偿 ✅**：`recipe.lens_vignette / lens_vignette_mid`（-100..100 /
  0..100，clamp 齐全）；引擎 `apply_vignette`（render.rs）——**线性光域**
  径向增益 `1 + k·rⁿ`，midpoint 经指数 0.6..3.0 控制作用范围，apply_develop
  第 0 步（tone 前），预览/导出/母版三路径共享；GUI「镜头校正 · Lens」区
  两滑杆；XMP `VignetteAmount`（键名从用户 140 份真边车实证）+
  `VignetteMidpoint`（ACR 文档配对键，用户边车中无非零实例，语义待真 LR
  验证），amount=0 时零键写出（与旧 writer 字节兼容）。单测：中心不动/
  径向单调/负值压暗/高中点收缩作用域；XMP 条件写出。
- **手动畸变校正 ✅（C2，2026-07-06 深夜）**：`recipe.lens_distortion`
  （-100..100，ACR 语义：正修桶形、负修枕形）；引擎（render.rs）
  `distort_norm / undistort_norm / apply_lens_distortion`——半对角线归一的
  单系数径向模型 `r_src = s·r·(1+k(sr)²)`，`k = −amount/100·0.25`（|k|<1/3
  保单调可逆；方向经两条独立推导交叉验证），负 amount 走 Newton 填满缩放
  （无黑角，同拉直的 auto-fill 策略）、正 amount 角部内容自然裁出；逆映射
  Newton 求三次根、被裁内容钳到单调极限落在视野外。管线插入点：三条路径
  （RAW 导出/baked/GUI redevelop）统一 develop 之后、拉直之前。GUI 映射链
  `view_norm_to_orig/orig_norm_to_view/geom_to_view` 全部带 `dist` 项
  （wb 吸管/范围取样/画笔/mask 放置/region/克隆 全调用点接入），镜头面板
  第三滑杆；XMP `LensManualDistortionAmount`（键名从用户 148 份真边车实证，
  仅非零写出）。已知近似：amount→k 增益是我方标定（Adobe 未公开），同数值
  下 LR 的校正强度可能不同——入真机验收单。单测：映射双向 roundtrip
  （4 幅度）/方向性/双符号无黑角/中心不动点/内容径向外移。
- **未做**：per-lens profile 校正（lensfun / 厂商 k1+k2 多项式——手动滑杆
  已覆盖目测校正，按镜头自动化留长期项）；去紫边（需边缘邻近门控，防误伤
  紫色主体）；透视 Upright。
- AI advisor 暂不暴露镜头字段（校正是测量性操作，非审美建议；schema 未加）。

### D. 色彩管理（✅ sRGB ICC + D2 广色域输出均完成 2026-07-06）
- **导出嵌 ICC ✅**：`render_to_file` 三种格式全部显式编码器 + `tag_icc`
  （render.rs，原 tag_srgb 泛化）——JPEG=APP2 ICC_PROFILE 段、PNG=iCCP 块、
  TIFF=tag 34675；profile 用 saucecontrol/Compact-ICC-Profiles
  （**CC0-1.0 公有领域**，assets/ 下入库；下载时验证 acsp 签名 +
  repo license API 实证）。单测逐格式验证 marker 字节存在。image 0.25.10
  三个编码器的 `set_icc_profile` 实现已核对（真存储非 Unsupported）。
- **D2 P3/AdobeRGB 输出 ✅（2026-07-06 深夜）**：`ExportColorSpace`
  {Srgb, DisplayP3, AdobeRgb} 入 `ExportOpts`（默认 Srgb，旧调用零变化）；
  **真 gamut 变换**（render.rs `convert_export_color_space`）——解 sRGB
  TRC → 线性光 3×3 原色变换 → 目标 TRC（P3 同 sRGB 曲线；AdobeRGB 纯
  563/256 gamma）；矩阵**运行时从原色色度推导**（`rgb_to_xyz`/`inv3`，
  不手抄七位小数表），三空间共 D65 白点、无色适应项；白点保持单测端到端
  锁定推导。profile：`DisplayP3-v2-magic.icc`（736 B）+
  `AdobeCompat-v2.icc`（374 B），下载时同样验 acsp+尺寸。GUI 导出面板
  「色彩空间」下拉（sRGB/Display P3/Adobe RGB），入 Prefs（越界回落
  sRGB）。未知扩展名（无法带 tag 的格式）刻意留 sRGB——P3/AdobeRGB 数值
  不带 profile 到处都显示错。单测：白/灰/中性保持、逆矩阵 roundtrip、
  sRGB 红在 P3 内（正 g/b）/在 AdobeRGB 是重缩放纯红（共享红原色）、
  JPEG/TIFF 文件字节含完整目标 profile。
- **未做**：egui 显示端色管（上游限制）；retouch 母版 PNG 的 ICC（工作
  文件，导出时会再过 render_to_file 补 tag）；工作空间本身仍是 sRGB
  （引擎在更宽空间显影是另一级大工程，超出导出选项范畴）。

### E. 1:1 真像素检查（✅ 已完成 2026-07-06）
- 现状（旧）：预览固定 1280px（gui.rs `PREVIEW_EDGE`），「1:1」= 预览像素。
- **已实现**（gui.rs）：标题行 Fit/1:1 旁新增预览分辨率下拉
  （1280 流畅 / 2560 / 4096 检查），入 `Prefs` 持久化（恢复时白名单校验，
  防坏存档造出 1px/100MP 预览）；`open_path` 按选中值缩放工作预览；
  **切换即重解码当前照片且配方保留**（复用批次 B 的 `keep_recipe` 通路），
  busy 时下拉禁用。代价如实标注：2560/4096 下每次滑杆调整变慢
  （develop_preview 逐像素成本 ×4/×10）。
- 未做（大工程，暂缓）：全分辨率 tile 金字塔（真 61MP 1:1 平滑漫游）。

### F. 导出管线（✅ 已完成 2026-07-06）
- 现状（旧）：`render_to_file` 只出全分辨率 16-bit TIFF / q95 JPEG；
  批量只同步配方不出图。
- **已实现**：
  - 引擎（render.rs）：`ExportOpts { long_edge, sharpen, jpeg_quality }` 作
    `render_to_file` 第 5 参（`Option`，None=旧行为，main.rs/serve.rs 7 个
    调用点传 None）；顺序=重采样（Lanczos3，永不放大）→ 输出锐化
    （luma unsharp r=1，在**缩放后**像素上）→ 按质量编码；返回保存后尺寸。
    单测锁定：50 长边出 50×25 且文件实测一致、超源尺寸不放大、q30<q95。
  - GUI（gui.rs）：导出区新增 长边下拉（原尺寸/1600/2048/2560/3840/5120）+
    输出锐化滑杆 + JPEG 质量滑杆（选 JPEG 时显示）；三项进 `Prefs` 持久化
    （Prefs 补 `serde(default)`+手写 Default 对齐 app 默认，旧存档不失效）；
    单张 Export/Download 与批量共用 `export_opts()`。
  - **批量渲染**（gui.rs `start_batch_render`）：gallery 多选 →「🖼 渲染
    选中(N)」——每张读它自己的 `./out/<stem>.recipe.json`（无则中性显影）
    按当前格式+导出选项出 `./out/<stem>.developed.*`；单 worker 顺序跑
    （61MP 全幅并行只会抖内存）；汇总成功/失败走 toast。AI Denoise 明确
    不参与批量（GPU sidecar 每张数分钟）。
- 未做（定位内暂缓）：水印、导出预设、色彩空间选项（后者归 §D）。

### G. 历史/版本（✅ 已完成 2026-07-06）
- 现状（旧）：undo/redo 100 步（内存态，关会话即失）；./out recipe json 单份。
- **已实现**（gui.rs）：版本快照 = `./out/<stem>.v<N>.recipe.json`（编号
  递增，不碰工作用 `<stem>.recipe.json`，库只读不变）；develop 面板
  「版本 · Versions」区——「＋ 存为版本」写下一号快照，列表每行「载入」
  替换当前参数（走 dirty→redevelop，撤销一步回到载入前）；列表缓存于
  `self.versions`，照片打开/存版时 `refresh_versions` 重扫（不逐帧扫
  ./out）；载入时 clamp() 防手改 JSON 越界。
- 未做（内存 undo 持久化到磁盘的完整历史——快照已覆盖"多套参数并存"
  的主需求，全量历史留给需要时再做）。

### UX 批次（用户指定方向 2026-07-07 起：UI 与操作细节）
- **第一批 ✅（2026-07-07）**：
  1. **蒙版覆盖叠加显示**（LR 的 O 叠加）——引擎新增 `render::mask_coverage`
     （与 apply_masks 完全同源：geometry×inversion×amount×range，range 在
     masks-cleared develop 参考上求值，单测锁定）；GUI 选中蒙版即显示红色
     半透明覆盖层，经畸变+拉直同一几何链落到 view，随滑杆/选中/O 键实时
     刷新。**同时关闭了"位图蒙版只有徽标"的 A② 已知边界**——位图/参数/
     范围蒙版统一走真实权重显示。开销如实：叠加开启+选中蒙版时每次滑杆
     调整多跑一次 masks-cleared develop（1280 下无感，4096 下可 O 关掉）。
  2. **削波警告**（LR 的 J）——红=任一通道 ≥254 溢出、蓝=全通道 ≤1 死黑，
     按**显影后导出像素**判定；标题行 ▲ 按钮 + J 键，入 Prefs 持久化。
  3. **Esc 统一退出工具**——裁剪/放置蒙版/WB 吸管/范围取样/图章/画笔/
     框选一键全退（画布与取样点保留可续）。
- **第二批 ✅（2026-07-07）**：
  4. **蒙版行 hover 即预览覆盖**——鼠标悬停蒙版列表任意行，图上即显示
     该蒙版的覆盖（不必点选；移开回落到选中项）。靠第一批的参考缓存，
     hover 切换只重算轻量覆盖图。
  5. **直方图削波三角灯**（LR 同款）——直方图左上/右上三角：暗部/亮部
     极值 bin 有像素时点亮（蓝/红），干净时灰；点击 = 切换 J 叠加，
     与 ▲ 按钮/J 键三入口同一状态。
  6. **批量渲染进度条**——worker 逐张上报 `Msg::BatchProgress`，顶栏
     实时进度条 + 状态行计数（此前只有结束 toast，跑长批像卡死）。
  - 滑杆微调核实为 egui 原生已有（点数值可键入、拖数值即精调），不另做。
- **第三批 ✅（2026-07-07）**：
  7. **叠加参考缓存失效收窄**——cache key 里中和 straighten/distortion/
     crop（develop_preview 不读它们，由调用方在其后应用；lens_vignette
     保留因为它是 develop 阶段）——拖拉直/畸变滑杆不再无谓重建参考。
  8. **工具光标语言补全**——平移=抓手（按住空格=Grab/拖动=Grabbing）、
     画笔/放置蒙版/裁剪=十字线；WB/范围/图章取源原有十字线保留。
  9. **蒙版 ⬆⬇ 排序**——蒙版顺序是渲染语义（顺序叠加，后面的范围蒙版
     看到前面的输出），选中行两键上移/下移，动了即 redevelop。
- **第四批 ✅（2026-07-07）**：
  10. **直方图削波三角按通道显色**（LR 同款）——三角颜色 = 极值 bin 里
      哪些通道有像素的加色混合：单通道即原色、双通道黄/品红/青、三通道
      全溢出为白（一眼区分中性压黑/溢出 vs 偏色），tooltip 列出具体通道。
  11. **蒙版真拖拽排序**——列表行为 egui 原生 dnd drag source（拖起浮影
      跟随光标），悬停行中线上下显示插入线，松手落位；`reorder_move`
      （remove+insert 的索引重映射）被单测对每个 (from, insert) 组合与
      真实 vec 操作逐元素比对锁定；拖拽中 `hovered()` 全局为 false，
      hover 预览自动暂停不churn覆盖层；⬆⬇ 按钮保留作精确路径。
  12. **裁剪柄方向光标**——悬停/拖动角柄显示对角线 Resize 光标
      （TL/BR ↘、TR/BL ↙），框内显示 Move；命中判定与 drag 同一
      `pick_handle`（同 12px 半径），不再全程十字线。
- **第五批 ✅（2026-07-07，真机反馈驱动：①窗口缩放挡按钮 ②蒙版体验
  ③操作手感）**：
  13. **顶栏换行不裁剪**——两行工具栏由 `ui.horizontal` 改
      `horizontal_wrapped`（原实现窗口一窄右侧按钮直接被裁掉、无法触达）。
      egui 换行只对**原子控件分配**生效，嵌套 `add_enabled_ui` 作用域在行尾
      会被压扁而非换行，故禁用门控改为逐控件 `ui.add_enabled`；导出**设置**
      （格式/长边/锐化/色彩空间）不再随无照片禁用（本就是持久化偏好），
      只有 Export/Download/Save XMP **动作**门控。另加
      `with_min_inner_size(980×620)` 兜底。
  14. **蒙版图上直接编辑**（LR 手势，不再"重画"从头拖）——选中蒙版显示
      可拖手柄：线性=zero 端/full 端/中点整体平移；径向=中心平移+四边
      中点单边调整（含最小尺寸保护）；写回经 `view_norm_to_orig` 同一
      几何链落回原始帧；拖动实时 redevelop+覆盖层刷新，松手由
      `commit_if_settled` 收为一步撤销；手柄命中优先于框选，框选拖拽
      进行中则框选优先（否则拖过手柄框会冻结）；手柄 hover/拖动
      Grab/Grabbing 光标；位图蒙版无参数手柄（维持徽标）。
  15. **放大后直接拖拽平移**（LR 手势，"手感别扭"主根源）——zoom>1 时
      主键拖拽即平移（原需空格/中键）；Ctrl+拖拽保留框选；激活工具/
      蒙版手柄/进行中的框选均优先于隐式平移；悬停即显抓手光标。
- 待做候选（未开工）：暂无——以真机验收反馈驱动。
- 快捷键现状：Ctrl+Z/Y/O/E/S、←/→ 走图、B 对比、O 叠加、J 削波、Esc 退出、
  空格/中键平移、**放大后直接拖拽=平移、Ctrl+拖拽=框选**、滚轮缩放、
  双击 Fit↔1:1、滑杆双击归零/点值键入、蒙版手柄拖拽=改形/移动。

### 明确不追（定位外）
- 图层/混合模式/智能对象、文字/矢量、设计合成——PS 的另一半；
  reimagine/fill + 反推配方已覆盖摄影侧的"创意改图"。

### 建议批次顺序（v0.3.x 起 · 2026-07-06 收官状态）
~~A①（范围蒙版）~~ ✅ → ~~B（双轨打通）~~ ✅ → ~~F（导出管线）~~ ✅ →
~~E（高分预览）~~ ✅ → ~~C（暗角 + C2 手动畸变）~~ ✅ →
~~D（sRGB ICC + D2 广色域输出）~~ ✅ → ~~G（版本）~~ ✅；
剩 A②（AI 分割）——待引擎位图 mask 通路，是差距清单最后一个大项。

## 完成每项后的例行动作

1. `cargo clippy --features gui --all-targets`（零警告）+ `cargo test --lib`
   + release build + GUI 启动烟雾。
2. 密钥扫描（`sk-[A-Za-z0-9]{20,}|OPENAI_API_KEY=|ANTHROPIC_API_KEY=`）后
   提交（结尾 `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`），
   用户说 push 才推、说发布才发 release。
3. 攒够一批（如 ①②③）可提议发 v0.3.0。
