use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};

use bevy::color::Srgba;
use serde::Deserialize;
use std::fs;

// --- 常量配置 ---
const TILE_SIZE: f32 = 40.0;
const SIM_TICK_RATE: f64 = 20.0; // 逻辑帧率 20 TPS
const BELT_SPEED: f32 = 0.5; // items per second
const BUFFER_CAPACITY: u32 = 50; // 机器缓存上限

// ==========================================
// 1. JSON 反序列化用的中间结构 (Raw Data)
// ==========================================

#[derive(Deserialize, Debug, Clone)]
struct RecipeItem {
    item: String,
    count: u32,
}

#[derive(Deserialize, Debug)]
struct RawRecipe {
    inputs: Vec<RecipeItem>,
    outputs: Vec<RecipeItem>,
    time: f32,
}
// 放在 RawRecipe 定义附近
impl Into<Recipe> for RawRecipe {
    fn into(self) -> Recipe {
        Recipe {
            inputs: self.inputs,
            outputs: self.outputs,
            time: self.time,
        }
    }
}

#[derive(Deserialize, Debug)]
struct RawItem {
    id: String,
    color: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize, Debug)]
struct RawMachine {
    id: String,
    width: u32,
    length: u32,
    color: String,
    layout: Vec<String>, // ["111000222"]
    recipes: Vec<RawRecipe>,

// ✅ 新增：速度倍率
    // 使用 default 属性，如果 JSON 里没写这一行，默认就是 1.0
    #[serde(default = "default_speed_modifier")]
    speed_modifier: f32,
}

// ✅ 辅助函数：定义默认速度
fn default_speed_modifier() -> f32 {
    1.0
}

#[derive(Deserialize, Debug, Clone)]
pub struct RawBelt {
    pub id: String,
    pub color: String,

    // items per second
    pub speed: f32,

    // 端口定义 (使用相对方向)
    // 默认: 从后面进 (Back)，往前面出 (Front)
    #[serde(default = "default_belt_inputs")]
    pub inputs: Vec<RelativeSide>,

    #[serde(default = "default_belt_outputs")]
    pub outputs: Vec<RelativeSide>,
}
// Serde 默认值辅助函数
fn default_belt_inputs() -> Vec<RelativeSide> {
    vec![RelativeSide::Back]
}
fn default_belt_outputs() -> Vec<RelativeSide> {
    vec![RelativeSide::Front]
}

#[derive(Debug, Clone)]
struct BeltPrototype {
    id: String,
    color: Color,
    speed: f32,
    inputs: Vec<RelativeSide>,
    outputs: Vec<RelativeSide>,
}

// ==========================================
// 2. 游戏运行时用的原型结构 (Runtime Data)
// ==========================================

#[derive(Debug, Clone)]
struct ItemPrototype {
    id: String,
    color: Color,
    description: String,
}

#[derive(Debug, Clone)]
struct Recipe {
    inputs: Vec<RecipeItem>,
    outputs: Vec<RecipeItem>,
    time: f32,
}

// --- 修改: Machine 原型 ---
// 机器端口定义 (预计算用)
#[derive(Debug, Clone)]
struct MachinePortDef {
    offset: IVec2,      // 相对坐标 (未旋转)
    side: RelativeSide, // 相对边缘
    r#type: PortType,   // Input 或 Output
}

#[derive(Debug, Clone)]
struct MachinePrototype {
    id: String,
    width: u32,
    height: u32, // 原 length
    color: Color,
    recipes: Vec<Recipe>,
    speed_modifier: f32,
    // 🔥 变更: 不再存 layout 矩阵，改为存端口列表
    ports: Vec<MachinePortDef>,
}

#[derive(Resource, Default)]
struct PrototypeLibrary {
    machines: HashMap<String, MachinePrototype>,
    items: HashMap<String, ItemPrototype>,
    belts: HashMap<String, BeltPrototype>, // ✅ 新增
}

// --- D. 基础枚举与类型 ---

// 必须 derive Deserialize 才能从 TOML 字符串 "North" 自动转换
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Direction {
    North,
    East,
    South,
    West,
}

// --- E. 场景配置结构 (Scenario Config) ---

#[derive(Deserialize)]
struct ScenarioMachineData {
    proto_id: String, // 机器原型 ID, 如 "refining_unit"
    x: i32,
    y: i32,
    #[serde(default = "default_direction")] // 可选字段，默认 North
    dir: Direction,
}

#[derive(Deserialize)]
struct ScenarioBeltData {
    x: i32,
    y: i32,
    in_dir: Direction,  // 流入方向
    out_dir: Direction, // 流出方向
    #[serde(default = "default_belt_id")] 
    proto_id: String,
}

// 辅助函数：默认值
fn default_belt_id() -> String {
    "basic-belt".to_string()
}

#[derive(Deserialize)]
struct ScenarioItemData {
    proto_id: String, // 物品原型 ID
    x: i32,
    y: i32,
    progress: f32, // 0.0 - 1.0
}

// --- F. 场景文件根结构 (scenario.toml) ---

#[derive(Deserialize)]
struct ScenarioConfig {
    // 使用 Option 是为了允许文件里某一部分完全为空
    machines: Option<Vec<ScenarioMachineData>>,
    belts: Option<Vec<ScenarioBeltData>>,
    items: Option<Vec<ScenarioItemData>>,
}

// 辅助函数：提供默认方向
fn default_direction() -> Direction {
    Direction::North
}

// 辅助函数
fn parse_hex(hex: &str) -> Color {
    let clean_hex = hex.trim_start_matches('#');
    match Srgba::hex(clean_hex) {
        Ok(srgba) => Color::from(srgba),
        Err(_) => {
            warn!("颜色解析失败: {}, 回退到白色", hex);
            Color::WHITE
        }
    }
}

// ✅ 核心加载函数 (修正版)
fn load_config(mut lib: ResMut<PrototypeLibrary>) {
    // --- Items 部分 (保持不变) ---
    let items_path = "assets/items.json";
    match fs::read_to_string(items_path) {
        Ok(content) => match serde_json::from_str::<Vec<RawItem>>(&content) {
            Ok(raw_items) => {
                for raw in raw_items {
                    lib.items.insert(
                        raw.id.clone(),
                        ItemPrototype {
                            id: raw.id,
                            color: parse_hex(&raw.color),
                            description: raw.description,
                        },
                    );
                }
                info!("✅ Items loaded: {}", lib.items.len());
            }
            Err(e) => error!("❌ items.json 格式解析失败: {}", e),
        },
        Err(e) => error!("❌ 无法读取 items.json: {}", e),
    }

    // 2. 加载 Belts (新增)
    let belts_path = "assets/belts.json";
    if let Ok(content) = fs::read_to_string(belts_path) {
        if let Ok(raw_belts) = serde_json::from_str::<Vec<RawBelt>>(&content) {
            for raw in raw_belts {
                lib.belts.insert(
                    raw.id.clone(),
                    BeltPrototype {
                        id: raw.id,
                        color: parse_hex(&raw.color),
                        speed: raw.speed,
                        inputs: raw.inputs,
                        outputs: raw.outputs,
                    },
                );
            }
            info!("✅ Belts loaded: {}", lib.belts.len());
        }
    } else {
        warn!("⚠️ 未找到 belts.json，传送带将无法工作");
    }

    // 3. 加载 Machines (重构)
    let machines_path = "assets/machines.json";
    match fs::read_to_string(machines_path) {
        Ok(content) => {
            if let Ok(raw_machines) = serde_json::from_str::<Vec<RawMachine>>(&content) {
                for raw in raw_machines {
                    // 逻辑映射：raw.length 是宽(X), raw.width 是高(Y)
                    let width = raw.length;
                    let height = raw.width;

                    let mut ports = Vec::new();

                    if let Some(mask_str) = raw.layout.first() {
                        let chars: Vec<char> = mask_str.chars().collect();
                        let stride = width as usize;

                        // 遍历字符矩阵
                        for (y, row_chars) in chars.chunks(stride).rev().enumerate() {
                            for (x, &char) in row_chars.iter().enumerate() {
                                let p_type = match char {
                                    '1' => PortType::Input,
                                    '2' => PortType::Output,
                                    _ => PortType::None,
                                };

                                if p_type != PortType::None {
                                    // 🔥 核心逻辑：根据坐标自动判断相对方向
                                    let side = calculate_relative_side(
                                        x,
                                        y,
                                        width as usize,
                                        height as usize,
                                    );

                                    ports.push(MachinePortDef {
                                        offset: IVec2::new(x as i32, y as i32),
                                        side,
                                        r#type: p_type,
                                    });
                                }
                            }
                        }
                    }

                    lib.machines.insert(
                        raw.id.clone(),
                        MachinePrototype {
                            id: raw.id,
                            width,
                            height,
                            color: parse_hex(&raw.color),
                            recipes: raw.recipes.into_iter().map(|r| r.into()).collect(), // 需实现 Into 或手动转换
                            speed_modifier: raw.speed_modifier,
                            ports, // ✅ 存入预计算结果
                        },
                    );
                }
                info!("✅ Machines loaded: {}", lib.machines.len());
            }
        }
        Err(e) => error!("❌ 无法读取 machines.json: {}", e),
    }
}

// 辅助：自动计算边缘方向
fn calculate_relative_side(x: usize, y: usize, w: usize, h: usize) -> RelativeSide {
    if y == h - 1 {
        return RelativeSide::Front;
    } // Top = Front (North)
    if x == w - 1 {
        return RelativeSide::Right;
    } // Right (East)
    if y == 0 {
        return RelativeSide::Back;
    } // Bottom = Back (South)
    if x == 0 {
        return RelativeSide::Left;
    } // Left (West)
    RelativeSide::Front // 默认 fallback
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelativeSide {
    Front, // 前 (实体的输出方向)
    Back,  // 后 (实体的输入方向)
    Left,  // 左
    Right, // 右
}

impl RelativeSide {
    /// 将相对方位转换为 "未旋转前" 的本地方向向量
    /// 假设实体默认面向 North
    pub fn to_local_direction(&self) -> Direction {
        match self {
            RelativeSide::Front => Direction::North, // 朝前 = 朝北
            RelativeSide::Back => Direction::South,  // 朝后 = 朝南
            RelativeSide::Left => Direction::West,   // 朝左 = 朝西
            RelativeSide::Right => Direction::East,  // 朝右 = 朝东
        }
    }
}

impl Direction {
    fn to_ivec2(&self) -> IVec2 {
        match self {
            Direction::North => IVec2::new(0, 1),
            Direction::East => IVec2::new(1, 0),
            Direction::South => IVec2::new(0, -1),
            Direction::West => IVec2::new(-1, 0),
        }
    }

    // 旋转向量 (用于计算机器端口的世界坐标)
    // size: (width, height)
    fn rotate_point(&self, local: IVec2, size: IVec2) -> IVec2 {
        match self {
            Direction::North => local,
            Direction::East => IVec2::new(local.y, size.x - 1 - local.x),
            Direction::South => IVec2::new(size.x - 1 - local.x, size.y - 1 - local.y),
            Direction::West => IVec2::new(size.y - 1 - local.y, local.x),
        }
    }

    fn opposite(&self) -> Self {
        match self {
            Direction::North => Direction::South,
            Direction::East => Direction::West,
            Direction::South => Direction::North,
            Direction::West => Direction::East,
        }
    }
}

// --- 工具：网格数学计算 ---
struct GridMath;

impl GridMath {
    // 将机器的局部端口坐标转换为世界坐标和方向
    pub fn transform_port(
        entity_pos: IVec2,
        entity_facing: Direction,
        entity_size: IVec2,
        local_offset: IVec2,
        relative_side: RelativeSide,
    ) -> (IVec2, Direction) {
        let rotated_offset = Self::rotate_point(
            local_offset,
            entity_facing,
            entity_size,
        );
        let world_pos = entity_pos + rotated_offset;

        let local_dir = relative_side.to_local_direction();
        let world_dir = Self::rotate_direction(local_dir, entity_facing);

        (world_pos, world_dir)
    }

    // 旋转点 (0,0) based, size 用于确定旋转中心
    pub fn rotate_point(p: IVec2, dir: Direction, size: IVec2) -> IVec2 {
        let (x, y) = (p.x, p.y);
        let (w, h) = (size.x, size.y);
        match dir {
            Direction::North => IVec2::new(x, y),
            Direction::East => IVec2::new(y, w - 1 - x), // 宽变高，高变宽
            Direction::South => IVec2::new(w - 1 - x, h - 1 - y),
            Direction::West => IVec2::new(h - 1 - y, x),
        }
    }

    pub fn rotate_direction(original: Direction, rotation: Direction) -> Direction {
        let dirs = [Direction::North, Direction::East, Direction::South, Direction::West];
        let idx_orig = dirs.iter().position(|&d| d == original).unwrap();
        let idx_rot = dirs.iter().position(|&d| d == rotation).unwrap();
        dirs[(idx_orig + idx_rot) % 4]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PortType {
    None,
    Input,
    Output,
}

// --- 核心组件 (ECS Components) ---

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GridPos(IVec2);

// 物品组件
#[derive(Component, Debug)]
struct Item {
    item_type: String, // 例如 "IronOre"
    // 视觉平滑插值用：记录上一帧的位置和目标位置
    visual_progress: f32,
}

// 传送带路径 (单条车道)
#[derive(Clone, Debug)]
struct BeltPath {
    input_dir: Direction,  // 来源方向
    output_dir: Direction, // 去向方向
    item: Option<Entity>,  // 当前持有的物品
    progress: f32,         // 0.0 -> 1.0
}

// 传送带组件
#[derive(Component, Debug)]
struct ConveyorBelt {
    // 支持重叠：一个格子上最多两条路径
    paths: Vec<BeltPath>,
}

// 机器状态组件
#[derive(Component, Debug)]
struct Machine {
    prototype_id: String, // 关联原型数据
    direction: Direction,

    // 生产状态
    progress: f32, // 当前配方生产进度 (秒)
    state: MachineState,

    // 缓存区 (Key: ItemType, Value: Count)
    input_buffer: HashMap<String, u32>,
    output_buffer: HashMap<String, u32>,

    // 轮询输出游标
    next_output_idx: usize,
    current_recipe_idx: usize,
}

// 定义标记组件
#[derive(Component)]
struct MachineLabel;

#[derive(Debug, Clone, Copy, PartialEq)]
enum MachineState {
    Idle,       // 缺料或空闲
    Working,    // 生产中
    OutputFull, // 出口堵塞
}

// --- 资源定义 (Resources) ---

// 地图索引：用于快速查找某个格子上有什么
#[derive(Resource, Default)]
struct GridMap {
    // 存储 Entity ID
    entities: HashMap<IVec2, Entity>,
}

// 补充 Direction 的旋转方法
impl Direction {
    fn rotate_clockwise(&self) -> Self {
        match self {
            Direction::North => Direction::East,
            Direction::East => Direction::South,
            Direction::South => Direction::West,
            Direction::West => Direction::North,
        }
    }
    fn rotate_counter_clockwise(&self) -> Self {
        self.rotate_clockwise().opposite()
    }
}

// --- 主程序入口 ---

fn main() {
    App::new()
        // Bevy 0.18 配置
        .add_plugins(DefaultPlugins.set(ImagePlugin::default_nearest()))
        .insert_resource(ClearColor(Color::srgb(0.9, 0.9, 0.9)))
        // 注册资源
        .init_resource::<GridMap>()
        .init_resource::<PrototypeLibrary>()
        // 设置固定时间步长 (20 TPS)
        .insert_resource(Time::<Fixed>::from_hz(SIM_TICK_RATE))
        .add_systems(Startup, (load_config, setup_scenario).chain())
        // 核心仿真循环 (FixedUpdate)
        .add_systems(
            FixedUpdate,
            (
                tick_belts_movement, // 1. 传送带内部移动
                tick_belt_to_belt,   // 2. 传送带/机器交互 (拓扑传输)
                tick_belt_to_machine,
                tick_machines_process, // 3. 机器生产逻辑
                tick_machines_output,  // 4. 机器输出
            )
                .chain(),
        )
        // .insert_resource(ClearColor(Color::BLACK))
        // 渲染同步 (Update)
        .add_systems(Update, (sync_visuals,))
        .run();
}

// --- 系统实现 ---

// 2. 初始化世界 (放置几个测试物体)
// 替代 setup_world
fn setup_scenario(
    mut commands: Commands,
    mut map: ResMut<GridMap>,
    asset_server: Res<AssetServer>,
    proto_lib: Res<PrototypeLibrary>,
) {
    // 1. 相机
    commands.spawn((
        Camera2d::default(),
        Transform::from_xyz(0.0, -100.0, 500.0).with_scale(Vec3::splat(1.0)),
    ));

    // 2. 读取 TOML
    let path = "assets/scenario.toml";
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            error!("❌ 严重错误: 无法读取 scenario.toml: {}", e);
            return;
        }
    };

    let config: ScenarioConfig = match toml::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            error!("❌ 严重错误: scenario.toml 格式解析失败: {}", e);
            return;
        }
    };

    // 3. 生成机器
    if let Some(machines) = config.machines {
        for m in machines {
            if proto_lib.machines.contains_key(&m.proto_id) {
                // 使用新的 spawn_machine (带 dir 参数)
                spawn_machine(
                    &mut commands,
                    &mut map,
                    &proto_lib,
                    &asset_server,
                    IVec2::new(m.x, m.y),
                    m.proto_id,
                    m.dir, // 👈 读取 TOML 里的方向
                );
            } else {
                error!("❌ Scenario 引用了不存在的机器 ID: {}", m.proto_id);
            }
        }
    }

    // 4. 生成传送带
    if let Some(belts) = config.belts {
        for b in belts {
            spawn_belt(
                &mut commands,
                &mut map,
                &proto_lib,
                IVec2::new(b.x, b.y),
                b.in_dir,
                b.out_dir,
                &b.proto_id,
            );
        }
    }

    // 5. 生成初始物品
    if let Some(items) = config.items {
        for i in items {
            // 获取颜色
            let color = proto_lib
                .items
                .get(&i.proto_id)
                .map(|p| p.color)
                .unwrap_or_else(|| {
                    warn!("⚠️ 物品 ID [{}] 未定义，使用默认颜色", i.proto_id);
                    Color::WHITE
                });

            let item_ent = commands
                .spawn((
                    Sprite {
                        color,
                        custom_size: Some(Vec2::new(20.0, 20.0)),
                        ..default()
                    },
                    Transform::from_xyz(0.0, 0.0, 1.0),
                    Item {
                        item_type: i.proto_id.clone(), // 👈 这里才是真正使用了 TOML 里的 ID
                        visual_progress: 0.0,
                    },
                ))
                .id();

            // 放入传送带
            let pos = IVec2::new(i.x, i.y);
            if let Some(belt_ent) = map.entities.get(&pos) {
                commands
                    .entity(*belt_ent)
                    .entry::<ConveyorBelt>()
                    .and_modify(move |mut belt| {
                        if let Some(path) = belt.paths.first_mut() {
                            path.item = Some(item_ent);
                            path.progress = i.progress;
                        }
                    });
            } else {
                warn!("⚠️ 物品放置在 ( {}, {} ) 但那里没有传送带", i.x, i.y);
                // 如果没有传送带，最好销毁或者就这样留在地上
                commands.entity(item_ent).despawn();
            }
        }
    }

    info!("✅ 场景加载完成！");
}

// 辅助：生成传送带
// 辅助：生成传送带 (带白色箭头)
fn spawn_belt(
    commands: &mut Commands,
    map: &mut GridMap,
    proto_lib: &PrototypeLibrary,
    pos: IVec2,
    in_dir: Direction,
    out_dir: Direction,
    belt_proto_id: &str,
) {
    // 1. 获取原型 (安全检查)
    let proto = proto_lib
        .belts
        .get(belt_proto_id)
        .expect("Belt ID not found");

    // 2. 计算视觉旋转
    // 逻辑：箭头默认向右(East)，根据 out_dir 旋转
    let rotation_quat = match out_dir {
        Direction::East => Quat::IDENTITY,
        Direction::North => Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        Direction::West => Quat::from_rotation_z(std::f32::consts::PI),
        Direction::South => Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2),
    };

    let belt_ent = commands
        .spawn((
            Sprite {
                color: proto.color,
                custom_size: Some(Vec2::new(38.0, 38.0)), // 略小于40以留出缝隙
                ..default()
            },
            // Z=0.0 是基础层
            Transform::from_translation((pos.as_vec2() * TILE_SIZE).extend(0.0)),
            GridPos(pos),
            ConveyorBelt {
                prototype_id: belt_proto_id.to_string(),
                speed: proto.speed,
                paths: vec![BeltPath {
                    input_dir: in_dir,
                    output_dir: out_dir,
                    item: None,
                    progress: 0.0,
                    // 初始化时，如果 input 和 output 相反，说明是直行，否则是弯道
                    // 这里可以留空，后续系统会处理，或者默认为 out_dir 的反向
                }],
            },
            Visibility::Visible,
            InheritedVisibility::default(),
            GlobalTransform::default(),
        ))
        // 🏹 添加白色箭头 (作为子实体)
        .with_children(|parent| {
            // 创建一个旋转容器，包含箭头的三个部分
            parent
                .spawn((
                    Transform {
                        rotation: rotation_quat,               // 应用旋转
                        translation: Vec3::new(0.0, 0.0, 0.1), // Z=0.1 浮在传送带表面
                        ..default()
                    },
                    Visibility::Visible,
                    InheritedVisibility::default(),
                    GlobalTransform::default(),
                ))
                .with_children(|arrow| {
                    // ⚪ 部件 A: 箭杆 (Shaft) - 长条
                    arrow.spawn(Sprite {
                        color: Color::WHITE,
                        custom_size: Some(Vec2::new(20.0, 4.0)), 
                        ..default()
                    });

                    // ⚪ 部件 B: 上箭翼 (Upper Wing)
                    arrow.spawn((
                        Sprite {
                            color: Color::WHITE,
                            custom_size: Some(Vec2::new(10.0, 4.0)),
                            ..default()
                        },
                        // 位置在右侧(X=5)，向上偏(Y=4)，旋转 -45度
                        Transform {
                            translation: Vec3::new(5.0, 4.0, 0.0),
                            rotation: Quat::from_rotation_z(-std::f32::consts::FRAC_PI_4),
                            ..default()
                        },
                    ));

                    // ⚪ 部件 C: 下箭翼 (Lower Wing)
                    arrow.spawn((
                        Sprite {
                            color: Color::WHITE,
                            custom_size: Some(Vec2::new(10.0, 4.0)),
                            ..default()
                        },
                        // 位置在右侧(X=5)，向下偏(Y=-4)，旋转 +45度
                        Transform {
                            translation: Vec3::new(5.0, -4.0, 0.0),
                            rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_4),
                            ..default()
                        },
                    ));
                    
                    // ❌ 已删除：此处你原来重复生成了一次箭杆，已移除
                });
        })
        .id();

    // 3. 注册到地图索引
    map.entities.insert(pos, belt_ent);
}

// 辅助：生成机器
// 修改后的 spawn_machine
fn spawn_machine(
    commands: &mut Commands,
    map: &mut GridMap,
    proto_lib: &PrototypeLibrary,
    asset_server: &AssetServer,
    pos: IVec2,
    proto_id: String,
    dir: Direction,
) {
    let proto = proto_lib.machines.get(&proto_id).unwrap();
    // 1. 获取旋转后的尺寸 (用于占据地图格子)
    let occupied_size = proto.get_size(dir);

    // 2. 视觉旋转 (用于 Sprite)
    let rotation = match dir {
        Direction::North => Quat::IDENTITY,
        Direction::East => Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2),
        Direction::South => Quat::from_rotation_z(std::f32::consts::PI),
        Direction::West => Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
    };

    // ... (中间的 recipe_text 代码保持不变) ...
    let recipe_text = if let Some(recipe) = proto.recipes.get(0) {
        let in_name = recipe
            .inputs
            .get(0)
            .map(|i| i.item.clone())
            .unwrap_or("None".to_string());
        let out_name = recipe
            .outputs
            .get(0)
            .map(|i| i.item.clone())
            .unwrap_or("None".to_string());
        format!("In: {}\nOut: {}", in_name, out_name)
    } else {
        "No Recipe".to_string()
    };

    // 3. 生成实体
    let machine_ent = commands
        .spawn((
            Sprite {
                color: Color::srgb(0.2, 0.2, 0.8),
                // 注意：这里使用原始宽高的 Box 即可，因为父级 Transform 会旋转它
                custom_size: Some(
                    Vec2::new(proto.width as f32, proto.length as f32) * TILE_SIZE - 2.0,
                ),
                ..default()
            },
            Transform {
                // 计算中心点偏移：(W/2, L/2)
                // 无论旋转与否，Sprite 都是基于自身局部坐标系的，所以用原始宽高
                translation: ((pos.as_vec2()
                    + Vec2::new(proto.width as f32 / 2.0, proto.length as f32 / 2.0))
                    * TILE_SIZE
                    - Vec2::splat(TILE_SIZE * 0.5))
                .extend(0.1),
                rotation,
                ..default()
            },
            GridPos(pos),
            Machine {
                prototype_id: proto_id.clone(),
                direction: dir,
                progress: 0.0,
                state: MachineState::Idle,
                input_buffer: HashMap::new(),
                output_buffer: HashMap::new(),
                next_output_idx: 0,
                current_recipe_idx: 0,
            },
            // ... Visibility 等其他组件 ...
            Visibility::Visible,
            InheritedVisibility::default(),
            GlobalTransform::default(),
        ))
        .with_children(|parent| {
            // ... (指示器生成代码保持不变) ...
            for y in 0..proto.length {
                for x in 0..proto.width {
                    let port_type = proto.layout[y as usize][x as usize];
                    if port_type != PortType::None {
                        // 重新计算中心偏移，确保指示器位置正确
                        let center_offset_x = (proto.width as f32 - 1.0) / 2.0;
                        let center_offset_y = (proto.length as f32 - 1.0) / 2.0;
                        let local_x = (x as f32 - center_offset_x) * TILE_SIZE;
                        let local_y = (y as f32 - center_offset_y) * TILE_SIZE;

                        let (color, size) = match port_type {
                            PortType::Input => (Color::srgb(0.0, 1.0, 0.0), Vec2::new(30.0, 10.0)),
                            PortType::Output => (Color::srgb(1.0, 0.0, 0.0), Vec2::new(30.0, 10.0)),
                            _ => (Color::WHITE, Vec2::ZERO),
                        };
                        parent.spawn((
                            Sprite {
                                color,
                                custom_size: Some(size),
                                ..default()
                            },
                            Transform::from_xyz(local_x, local_y, 0.1),
                        ));
                    }
                }
            }
            // 调试文字
            parent.spawn((
                Text2d::new(recipe_text),
                TextFont {
                    font: asset_server.load("fonts/FiraSans-Bold.ttf"),
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                Transform::from_xyz(0.0, 30.0, 2.0),
                MachineLabel,
            ));
        })
        .id();

    // ✅✅✅ 关键修复：循环填充 GridMap ✅✅✅
    // 根据旋转后的尺寸，填充所有被占据的格子
    for x in 0..occupied_size.x {
        for y in 0..occupied_size.y {
            let tile_pos = pos + IVec2::new(x, y);
            map.entities.insert(tile_pos, machine_ent);
        }
    }
}
// --- 核心逻辑系统 ---

// 3. 传送带内部移动 (Backpressure 实现)
fn tick_belts_movement(time: Res<Time>, mut belts: Query<&mut ConveyorBelt>) {
    let dt = time.delta_secs();

    for mut belt in belts.iter_mut() {
        for path in belt.paths.iter_mut() {
            if path.item.is_some() {
                // 移动物品
                if path.progress < 1.0 {
                    path.progress += BELT_SPEED * dt;

                    // 钳位：如果堵塞，停在 1.0
                    if path.progress > 1.0 {
                        path.progress = 1.0;
                    }
                }
            }
        }
    }
}

// 4. 传输握手逻辑 (最复杂的部分)
fn tick_belt_to_belt(
    // 使用 Entity 来确保源和目标不同
    mut belt_query: Query<(Entity, &GridPos, &mut ConveyorBelt)>,
    map: Res<GridMap>,
) {
    // --- Phase 1: 收集传输请求 ---
    // 存储结构: (源实体, 目标实体, 源Path索引, 源输出方向)
    let mut transfers = Vec::new();

    for (entity, pos, belt) in belt_query.iter() {
        for (idx, path) in belt.paths.iter().enumerate() {
            // 只有当物品到达终点 (progress >= 1.0) 时才尝试传输
            if path.item.is_some() && path.progress >= 1.0 {
                let target_pos = pos.0 + path.output_dir.to_ivec2();

                // 检查目标位置是否有实体
                if let Some(&target_ent) = map.entities.get(&target_pos) {
                    // 这里我们记录下 source_output_dir，后面赋值给 target_input_dir 用
                    transfers.push((entity, target_ent, idx, path.output_dir));
                }
            }
        }
    }

    // --- Phase 2: 执行传输 ---
    for (src_ent, target_ent, src_idx, src_out_dir) in transfers {
        // 1. 基本检查：不能自环
        if src_ent == target_ent {
            continue;
        }

        // 2. 获取双方的可变引用
        if let Ok([mut src, mut target]) = belt_query.get_many_mut([src_ent, target_ent]) {
            let src_belt = &mut src.2;

            // 3. 再次确认源物品还在 (防止多重传输导致的冲突)
            if let Some(item_entity) = src_belt.paths[src_idx].item {
                let target_belt = &mut target.2;
                // 假设目标只有一条路径 (简单情况)，或者你需要更复杂的逻辑来选路径
                if let Some(target_path) = target_belt.paths.first_mut() {
                    // --- 🔥 核心修改：转弯逻辑 🔥 ---

                    // 条件 A: 目标必须是空的
                    let is_empty = target_path.item.is_none();

                    // 条件 B: 目标不能是"反向"的 (不能把东西传给一个正对着你吐东西的传送带)
                    // 例如: A -> East, B -> West. A不能传给B。
                    // src_out_dir (East) != target_path.output_dir.opposite() (West.opposite = East) -> False
                    let is_not_blocked = src_out_dir != target_path.output_dir.opposite();

                    if is_empty && is_not_blocked {
                        // === 执行转移 ===

                        // 1. 搬运物品实体
                        target_path.item = Some(item_entity);
                        target_path.progress = 0.0;

                        // 2. 🔥 关键：修改目标的 input_dir 以匹配来源！
                        // 告诉目标："这个货是从 src_out_dir 来的"
                        // 所以目标的 input_dir 应该是 src_out_dir 的反方向
                        // 例子：源向东(East)输出 -> 目标接收方向应设为西(West)
                        // 这样 sync_visuals 发现 input(West) != output(North) 就会画出直角弯
                        target_path.input_dir = src_out_dir.opposite();

                        // 3. 清空源头
                        src_belt.paths[src_idx].item = None;
                    }
                }
            }
        }
    }
}

fn tick_belt_to_machine(
    mut commands: Commands,
    mut belt_query: Query<(&GridPos, &mut ConveyorBelt)>,
    mut machine_query: Query<(&GridPos, &mut Machine)>,
    item_query: Query<&Item>,
    map: Res<GridMap>,
    proto_lib: Res<PrototypeLibrary>,
) {
    for (belt_pos, mut belt) in belt_query.iter_mut() {
        for path in belt.paths.iter_mut() {
            // 只有当传送带上有物品，且物品到达末端时才尝试传输
            if let Some(item_ent) = path.item {
                if path.progress >= 1.0 {
                    let target_pos = belt_pos.0 + path.output_dir.to_ivec2();

                    // 1. 检查目标位置是否有实体
                    if let Some(target_ent) = map.entities.get(&target_pos) {
                        // 2. 检查目标是否是机器
                        if let Ok((m_pos, mut machine)) = machine_query.get_mut(*target_ent) {
                            if let Some(proto) = proto_lib.machines.get(&machine.prototype_id) {
                                // 🔥 新的端口检查逻辑 🔥
                                let is_port_valid = proto
                                    .ports
                                    .iter()
                                    .filter(|p| p.r#type == PortType::Input) // 只看输入口
                                    .any(|p| {
                                        // 调用通用工具计算世界坐标
                                        let (world_pos, _world_dir) = GridMath::transform_port(
                                            m_pos.0,
                                            machine.direction,
                                            IVec2::new(proto.width as i32, proto.height as i32),
                                            p.offset,
                                            p.side,
                                        );
                                        world_pos == target_pos // 只比对位置
                                    });

                                if !is_port_valid {
                                    continue;
                                }

                                // --- 3. 检查配方和库存 ---
                                if let Ok(item_cmp) = item_query.get(item_ent) {
                                    if let Some(recipe) =
                                        proto.recipes.get(machine.current_recipe_idx)
                                    {
                                        // A. 配方匹配检查
                                        let is_needed = recipe
                                            .inputs
                                            .iter()
                                            .any(|req| req.item == item_cmp.item_type);

                                        if is_needed {
                                            let current_count = machine
                                                .input_buffer
                                                .get(&item_cmp.item_type)
                                                .copied()
                                                .unwrap_or(0);

                                            // B. 库存容量检查
                                            if current_count < BUFFER_CAPACITY {
                                                // ✅ 成功接收
                                                machine.input_buffer.insert(
                                                    item_cmp.item_type.clone(),
                                                    current_count + 1,
                                                );

                                                // 销毁传送带上的物品实体
                                                commands.entity(item_ent).despawn();
                                                path.item = None;

                                                info!(
                                                    "✅ 机器 [{}] 成功吃掉: {}",
                                                    machine.prototype_id, item_cmp.item_type
                                                );
                                            } else {
                                                // 库存已满 (静默失败，等待消耗)
                                            }
                                        } else {
                                            // 物品不对 (调试时可以打印，正式版通常静默堵塞)
                                            // debug!("⛔ 拒绝: 机器不需要 {}", item_cmp.item_type);
                                        }
                                    } else {
                                        // 机器没有设置配方
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
// 5. 机器生产逻辑
fn tick_machines_process(
    time: Res<Time<Fixed>>,
    mut query: Query<(Entity, &mut Machine)>,
    proto_lib: Res<PrototypeLibrary>,
) {
    let dt = time.delta_secs();

    for (entity, mut machine) in query.iter_mut() {
        if let Some(proto) = proto_lib.machines.get(&machine.prototype_id) {
            // 获取配方
            if let Some(recipe) = proto.recipes.get(machine.current_recipe_idx) {
                match machine.state {
                    MachineState::Idle => {
                        // --- 1. 检查原料 ---
                        let mut can_craft = true;
                        let mut missing_info = String::new(); // 用于记录缺什么，方便打印

                        for input_req in &recipe.inputs {
                            let current_count = machine
                                .input_buffer
                                .get(&input_req.item)
                                .copied()
                                .unwrap_or(0);

                            if current_count < input_req.count {
                                can_craft = false;
                                // 记录缺料详情
                                missing_info = format!(
                                    "{} (持有: {}, 需要: {})",
                                    input_req.item, current_count, input_req.count
                                );

                                // 为了防止控制台被“空机器”刷屏，我们只在“持有量 > 0 但不足”时打印日志
                                // 这样你就能立刻发现“是不是配方设置了需要5个但我只运进去1个”的问题
                                if current_count > 0 {
                                    info!("💤 机器 [{:?}] 原料不足: {}", entity, missing_info);
                                }
                                break;
                            }
                        }

                        // --- 2. 扣除原料 (安全版) ---
                        if can_craft {
                            for input_req in &recipe.inputs {
                                if let Some(current) = machine.input_buffer.get_mut(&input_req.item)
                                {
                                    if *current >= input_req.count {
                                        *current -= input_req.count;
                                    } else {
                                        error!(
                                            "❌ 逻辑错误: 机器 {:?} 原料 {} 检查通过但扣除失败！",
                                            entity, input_req.item
                                        );
                                    }
                                } else {
                                    error!(
                                        "❌ 逻辑错误: 机器 {:?} 缺少原料 Key: {}",
                                        entity, input_req.item
                                    );
                                }
                            }

                            machine.state = MachineState::Working;
                            machine.progress = 0.0;

                            // ✅ 打印开始生产日志
                            info!(
                                "⚙️ 机器 [{:?}] 开始生产 (配方耗时: {:.1}s)",
                                entity, recipe.time
                            );
                        }
                    }

                    MachineState::Working => {
                        machine.progress += dt;

                        // (可选) 打印进度调试，如果配方时间很长可以解开下面注释
                        // if machine.progress % 1.0 < dt { info!("...生产中 {:.1}s / {:.1}s", machine.progress, recipe.time); }

                        if machine.progress >= recipe.time {
                            // --- 3. 产出完成 ---
                            for output_prod in &recipe.outputs {
                                let current = machine
                                    .output_buffer
                                    .get(&output_prod.item)
                                    .copied()
                                    .unwrap_or(0);
                                machine
                                    .output_buffer
                                    .insert(output_prod.item.clone(), current + output_prod.count);

                                // ✅ 打印产出日志
                                info!(
                                    "✨ 机器 [{:?}] 生产完成! 产出: {} (+{}) | 当前库存: {}",
                                    entity,
                                    output_prod.item,
                                    output_prod.count,
                                    current + output_prod.count
                                );
                            }

                            // 检查堆积 (这里硬编码了 50 作为上限，之后可以改为常量配置)
                            let is_output_full =
                                machine.output_buffer.values().any(|&count| count >= 50);

                            if is_output_full {
                                machine.state = MachineState::OutputFull;
                                warn!("⚠️ 机器 [{:?}] 出口堵塞! 产物堆积已满，停止工作。", entity);
                            } else {
                                machine.state = MachineState::Idle;
                                machine.progress = 0.0;
                            }
                        }
                    }

                    MachineState::OutputFull => {
                        // 等待输出被拿走
                        let is_output_full =
                            machine.output_buffer.values().any(|&count| count >= 50);
                        if !is_output_full {
                            machine.state = MachineState::Idle;
                            machine.progress = 0.0;
                            info!("♻️ 机器 [{:?}] 堵塞解除，恢复工作。", entity);
                        }
                    }
                }
            } else {
                // 如果配方索引越界
                warn!(
                    "❌ 机器 [{:?}] 配方索引错误: {}",
                    entity, machine.current_recipe_idx
                );
            }
        }
    }
}
// 6. 视觉同步 (逻辑坐标 -> 屏幕像素)
// 6. 视觉同步 (逻辑坐标 -> 屏幕像素)
fn sync_visuals(
    belt_query: Query<(&GridPos, &ConveyorBelt)>,
    mut item_query: Query<(&mut Transform, &Item)>,
) {
    for (pos, belt) in belt_query.iter() {
        for path in belt.paths.iter() {
            if let Some(item_ent) = path.item {
                if let Ok((mut transform, _)) = item_query.get_mut(item_ent) {
                    let base_pos = pos.0.as_vec2() * TILE_SIZE;
                    let half_tile = TILE_SIZE * 0.5;

                    // 1. 计算起点 (Input Dir 是来源方向)
                    // 例如：Input=West (左)，物品应该从左边边缘出现
                    // Direction::West = (-1, 0).  Offset = (-0.5 * size, 0)
                    let start_offset = path.input_dir.to_ivec2().as_vec2() * half_tile;

                    // 2. 计算终点 (Output Dir 是去向方向)
                    let end_offset = path.output_dir.to_ivec2().as_vec2() * half_tile;

                    // 3. 线性插值
                    // 注意：这里我们反转 start_offset 的逻辑，如果 input 是 West，
                    // 意味着它来自于 West 格子，所以它在当前格子的 West 边缘。
                    // Bevy 的 Direction::West 是 (-1, 0)。
                    // 所以 start_offset = (-20, 0)，这正好是左边缘。逻辑正确。
                    
                    let current_offset = start_offset.lerp(end_offset, path.progress);

                    transform.translation = (base_pos + current_offset).extend(1.0);
                }
            }
        }
    }
}

// 7. 机器输出逻辑 (Machine -> Belt)
fn tick_machines_output(
    mut commands: Commands,
    mut machine_query: Query<(Entity, &GridPos, &mut Machine)>,
    mut belt_query: Query<&mut ConveyorBelt>,
    map: Res<GridMap>,
    proto_lib: Res<PrototypeLibrary>,
) {
    for (entity, m_pos, mut machine) in machine_query.iter_mut() {
        // 1. 快速检查：如果没有产物，直接跳过该机器
        if machine.output_buffer.is_empty() {
            continue;
        }

        let Some(proto) = proto_lib.machines.get(&machine.prototype_id) else { continue; };

        // 2. 找到第一个有库存的物品
        // Clone item_id 以便稍后使用，避免借用冲突
        let item_to_output = machine.output_buffer.iter()
            .find(|(_, &count)| count > 0)
            .map(|(k, _)| k.clone());

        if let Some(item_id) = item_to_output {
            let mut success = false; // ✅ 修复：变量定义在循环外
            
            // 筛选输出端口
            let output_ports = proto.ports.iter().filter(|p| p.r#type == PortType::Output);

            for p in output_ports {
                // 计算目标坐标
                let (port_pos, port_facing) = GridMath::transform_port(
                    m_pos.0,
                    machine.direction,
                    IVec2::new(proto.width as i32, proto.height as i32),
                    p.offset,
                    p.side,
                );

                let target_pos = port_pos + port_facing.to_ivec2();

                // 检查目标格子是否有实体
                if let Some(&target_ent) = map.entities.get(&target_pos) {
                    // 尝试获取传送带组件
                    if let Ok(mut belt) = belt_query.get_mut(target_ent) {
                        // 假设单车道：获取第一条路径
                        if let Some(path) = belt.paths.first_mut() {
                            // 🔥 核心逻辑：只有传送带完全空闲才喷射
                            // 防止物品重叠
                            if path.item.is_none() {
                                // === A. 生成物品实体 ===
                                let color = proto_lib.items.get(&item_id)
                                    .map(|i| i.color)
                                    .unwrap_or(Color::WHITE);
                                
                                let item_ent = commands.spawn((
                                    Sprite {
                                        color,
                                        custom_size: Some(Vec2::new(20.0, 20.0)),
                                        ..default()
                                    },
                                    // 初始位置设为端口位置
                                    Transform::from_translation(
                                        (port_pos.as_vec2() * TILE_SIZE).extend(1.0),
                                    ),
                                    Item {
                                        item_type: item_id.clone(),
                                        visual_progress: 0.0,
                                    },
                                )).id();

                                // === B. 放入传送带 ===
                                path.item = Some(item_ent);
                                // ✅ 设定初始进度：0.5 代表放在格子正中间，或者 0.0 代表从边缘进入
                                // 推荐 0.0 以获得平滑进入的动画，但在逻辑上要确保 belt_to_belt 处理得当
                                path.progress = 0.5; 
                                
                                // 设置传送带的流入方向，以便视觉正确绘制（物品看起来是从机器那个方向来的）
                                path.input_dir = port_facing.opposite();

                                success = true;
                            }
                        }
                    }
                }

                if success { break; } // 如果某个端口成功输出，停止尝试其他端口
            }

            // === C. 如果成功，扣除库存 ===
            if success {
                if let Some(c) = machine.output_buffer.get_mut(&item_id) {
                    *c -= 1;
                    info!("✅ 机器 [{:?}] 输出 {} 成功", entity, item_id);
                }
            }
        }
    }
}