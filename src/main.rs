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
    length: u32,   // ✅ 使用 length
    color: String,
    layout: Vec<String>, // ["111000222"]
    recipes: Vec<RawRecipe>,
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

#[derive(Debug, Clone)]
struct MachinePrototype {
    id: String,
    width: u32,
    length: u32, // ✅ 运行时也叫 length
    // layout[y][x] -> 
    // y 范围是 0..width (行数)
    // x 范围是 0..length (每行长度)
    layout: Vec<Vec<PortType>>, 
    color: Color,
    recipes: Vec<Recipe>,
}

#[derive(Resource, Default)]
struct PrototypeLibrary {
    machines: HashMap<String, MachinePrototype>,
    items: HashMap<String, ItemPrototype>,
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
}

#[derive(Deserialize)]
struct ScenarioItemData {
    proto_id: String, // 物品原型 ID
    x: i32,
    y: i32,
    progress: f32,    // 0.0 - 1.0
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
// ✅ 核心加载函数 (增强版)
fn load_config(mut lib: ResMut<PrototypeLibrary>) {
    // --- Items 部分 ---
    let items_path = "assets/items.json";
    match fs::read_to_string(items_path) {
        Ok(content) => {
            let raw_items: Vec<RawItem> = serde_json::from_str(&content).expect("Items JSON 格式错误");
            for raw in raw_items {
                lib.items.insert(raw.id.clone(), ItemPrototype {
                    id: raw.id,
                    color: parse_hex(&raw.color),
                    description: raw.description,
                });
            }
            info!("Items loaded: {}", lib.items.len());
        },
        Err(e) => error!("❌ 无法读取 items.json: {}", e), // 👈 报错提示
    }

    // --- Machines 部分 ---
    let machines_path = "assets/machines.json";
    match fs::read_to_string(machines_path) {
        Ok(content) => {
            // 使用 match 处理 JSON 解析错误，避免直接 panic 且无提示
            match serde_json::from_str::<Vec<RawMachine>>(&content) {
                Ok(raw_machines) => {
                    for raw in raw_machines {
                        // 1. 获取掩码字符串
                        if let Some(mask_str) = raw.layout.first() {
                            let chars: Vec<char> = mask_str.chars().collect();
                            let expected_len = (raw.width * raw.length) as usize;
                            
                            if chars.len() != expected_len {
                                error!("❌ 机器 [{}] 布局错误: width({}) * length({}) != layout长度({})", 
                                    raw.id, raw.width, raw.length, chars.len());
                                continue; 
                            }

                            // 2. 解析 Layout
                            let mut layout_matrix = Vec::new();
                            for row_chars in chars.chunks(raw.length as usize).rev() {
                                let mut row = Vec::new();
                                for &char in row_chars {
                                    row.push(match char {
                                        '1' => PortType::Input,
                                        '2' => PortType::Output,
                                        _ => PortType::None,
                                    });
                                }
                                layout_matrix.push(row);
                            }

                            let recipes = raw.recipes.into_iter().map(|r| Recipe {
                                inputs: r.inputs,
                                outputs: r.outputs,
                                time: r.time,
                            }).collect();

                            lib.machines.insert(raw.id.clone(), MachinePrototype {
                                id: raw.id,
                                width: raw.width,
                                length: raw.length,
                                layout: layout_matrix,
                                color: parse_hex(&raw.color),
                                recipes,
                            });
                        } else {
                            error!("❌ 机器 [{}] Layout 为空", raw.id);
                        }
                    }
                    info!("Machines loaded: {}", lib.machines.len());
                },
                Err(e) => error!("❌ machines.json JSON 解析失败: {}", e),
            }
        },
        Err(e) => error!("❌ 无法读取 machines.json: {}", e), // 👈 报错提示
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


// ----------------------------------------------------------------------
// 🔧 核心算法：局部坐标 -> 世界坐标旋转变换
// ----------------------------------------------------------------------

impl MachinePrototype {
    /// 获取机器在指定旋转方向下的 (宽, 高)
    /// 返回值: (X轴跨度, Y轴跨度)
    pub fn get_size(&self, dir: Direction) -> IVec2 {
        match dir {
            // 南北朝向：保持原样 (Length 是 X, Width 是 Y)
            Direction::North | Direction::South => IVec2::new(self.length as i32, self.width as i32),
            // 东西朝向：宽高互换
            Direction::East | Direction::West => IVec2::new(self.width as i32, self.length as i32),
        }
    }

    /// 获取旋转后的所有端口信息
    /// 返回: Vec<(世界坐标, 端口朝向)>
    pub fn get_ports_with_facing(
        &self, 
        origin: IVec2,   // 机器左下角的 GridPos
        facing: Direction, // 机器本身的朝向
        target_type: PortType // 只获取 Input 或 Output
    ) -> Vec<(IVec2, Direction)> {
        let mut ports = Vec::new();

        // 遍历原始 Layout 矩阵
        // y 是行索引 (0..width), x 是列索引 (0..length)
        for (y, row) in self.layout.iter().enumerate() {
            for (x, &p_type) in row.iter().enumerate() {
                if p_type == target_type {
                    let local_pos = IVec2::new(x as i32, y as i32);

                    // 1. 计算旋转后的局部坐标 offset
                    let rotated_offset = self.rotate_point(local_pos, facing);

                    // 2. 计算世界坐标
                    let world_pos = origin + rotated_offset;

                    // 3. 计算端口的"流向" (用于箭头显示和连接逻辑)
                    // 例如：顶部的 Input 端口，其流向应该是 pointing South (进入机器)
                    // 或者 Output 端口，其流向 pointing North (流出机器)
                    // 这里我们计算"朝外"的方向
                    let outward_dir = self.calculate_outward_dir(x, y, facing);
                    
                    ports.push((world_pos, outward_dir));
                }
            }
        }
        ports
    }

    /// 内部辅助：计算点在旋转后的位置
    /// 假设旋转中心是网格的 (0,0) 到 (L, W) 区域的整体旋转
    fn rotate_point(&self, p: IVec2, dir: Direction) -> IVec2 {
        let l = self.length as i32;
        let w = self.width as i32;
        let (x, y) = (p.x, p.y);

        // 顺时针 (Clockwise) 旋转逻辑
        match dir {
            Direction::North => IVec2::new(x, y),
            // 北(x,y) 转 东 -> x变成y轴, y变成倒转的x轴 (适应新尺寸 W x L)
            Direction::East  => IVec2::new(y, l - 1 - x), 
            // 北(x,y) 转 南 -> 倒转x, 倒转y (尺寸 L x W)
            Direction::South => IVec2::new(l - 1 - x, w - 1 - y),
            // 北(x,y) 转 西 -> y变成倒转x, x变成y (尺寸 W x L)
            Direction::West  => IVec2::new(w - 1 - y, x),
        }
    }

    /// 内部辅助：计算端口原本是朝哪边的，并加上机器旋转
    fn calculate_outward_dir(&self, x: usize, y: usize, machine_facing: Direction) -> Direction {
        // 1. 判断端口在原始布局的哪个边缘
        let l = self.length as usize;
        let w = self.width as usize;

        let local_dir = if y == w - 1 {
            Direction::North // 在顶部
        } else if x == l - 1 {
            Direction::East  // 在右侧
        } else if y == 0 {
            Direction::South // 在底部
        } else if x == 0 {
            Direction::West  // 在左侧
        } else {
            Direction::North // 默认 (内部端口)
        };

        // 2. 将边缘方向叠加机器的旋转
        self.rotate_direction(local_dir, machine_facing)
    }

    /// 旋转方向枚举
    fn rotate_direction(&self, original: Direction, rotation: Direction) -> Direction {
        // 简单的枚举映射
        match rotation {
            Direction::North => original,
            Direction::East => match original {
                Direction::North => Direction::East,
                Direction::East => Direction::South,
                Direction::South => Direction::West,
                Direction::West => Direction::North,
            },
            Direction::South => match original {
                Direction::North => Direction::South,
                Direction::East => Direction::West,
                Direction::South => Direction::North,
                Direction::West => Direction::East,
            },
            Direction::West => match original {
                Direction::North => Direction::West,
                Direction::East => Direction::North,
                Direction::South => Direction::East,
                Direction::West => Direction::South,
            },
        }
    }
}

// 补充 Direction 的旋转方法
impl Direction {
    fn rotate_clockwise(&self) -> Self {
        match self {
            Direction::North => Direction::East, Direction::East => Direction::South,
            Direction::South => Direction::West, Direction::West => Direction::North,
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
        
        .add_systems(Startup, (load_config, setup_world).chain())
        
        // 核心仿真循环 (FixedUpdate)
        .add_systems(FixedUpdate, (
            tick_belts_movement,      // 1. 传送带内部移动
            tick_belt_to_belt, // 2. 传送带/机器交互 (拓扑传输)
            tick_belt_to_machine,
            tick_machines_process,    // 3. 机器生产逻辑
            tick_machines_output,     // 4. 机器输出
        ).chain())
        // .insert_resource(ClearColor(Color::BLACK))
        // 渲染同步 (Update)
        .add_systems(Update, (
            sync_visuals,
        ))
        
        .run();
}

// --- 系统实现 ---


// 2. 初始化世界 (放置几个测试物体)
fn setup_world(
    mut commands: Commands, 
    mut map: ResMut<GridMap>,
    asset_server: Res<AssetServer>,
    proto_lib: Res<PrototypeLibrary>,
) {
    commands.spawn((
            Camera2d::default(),
            // Z = 999.0 确保相机在所有物体的前面
            // scale = 0.5 意味着放大 2 倍 (数值越小越放大)
            Transform::from_xyz(0.0, 0.0, 300.0).with_scale(Vec3::splat(0.5)), 
        ));
    // A. 放置一个传送带 (0,0) -> (1,0)
    spawn_belt(&mut commands, &mut map, IVec2::new(2, 2), Direction::North, Direction::South);
    spawn_belt(&mut commands, &mut map, IVec2::new(2, 1), Direction::North, Direction::South);
    spawn_belt(&mut commands, &mut map, IVec2::new(2, -3), Direction::North, Direction::South);
    spawn_belt(&mut commands, &mut map, IVec2::new(2, -4), Direction::North, Direction::South);
    spawn_belt(&mut commands, &mut map, IVec2::new(3, -3), Direction::West, Direction::East);
    
    // 在第一个传送带上放一个物品
    let item_ent = commands.spawn((
        Sprite {
            color: Color::srgb(1.0, 0.5, 0.5),
            custom_size: Some(Vec2::new(20.0, 20.0)),
            ..default()
        },
        Transform::default(),
        Item { item_type: "Amethyst Ore".to_string(), visual_progress: 0.0 },
    )).id();
    
    // 把物品挂载到 (0,0) 传送带的第一个路径上
    if let Some(belt_ent) = map.entities.get(&IVec2::new(2, 2)) {
        // ✅ 修正代码 (添加 move)
        commands.entity(*belt_ent).entry::<ConveyorBelt>().and_modify(move |mut belt| { // <--- 这里加 move
            belt.paths[0].item = Some(item_ent);
            belt.paths[0].progress = 0.5;
        });
    }

    // B. 放置一个机器 (2, -1) -> 这样它的左边 Input 刚好对着 (1,0) 的传送带
    // 3x3 机器，原点在左下角。Input 在 y=2。
    // 如果放置在 (2, -2)，则 y=2 (相对) -> 世界坐标 y=0。
    spawn_machine(&mut commands, &mut map, &proto_lib, IVec2::new(2, -2), "refining_unit".to_string());
}

// 辅助：生成传送带
// 辅助：生成传送带 (带白色箭头)
fn spawn_belt(
    commands: &mut Commands, 
    map: &mut GridMap, 
    pos: IVec2, 
    in_dir: Direction, 
    out_dir: Direction
) {
    // 1. 计算旋转角度 (基于输出方向)
    // 我们的箭头默认朝右 (East, 0度)
    // Bevy 的 2D 旋转是逆时针 (CCW)
    let rotation_quat = match out_dir {
        Direction::East  => Quat::IDENTITY,
        Direction::North => Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), // +90度
        Direction::West  => Quat::from_rotation_z(std::f32::consts::PI),        // 180度
        Direction::South => Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2),// -90度
    };

    let belt_ent = commands.spawn((
        Sprite {
            // 🔶 1. 传送带底座颜色：橙色
            color: Color::srgb(1.0, 0.5, 0.0), 
            custom_size: Some(Vec2::new(38.0, 38.0)), // 稍微留缝
            ..default()
        },
        Transform::from_translation((pos.as_vec2() * TILE_SIZE).extend(0.0)),
        GridPos(pos),
        ConveyorBelt {
            paths: vec![BeltPath {
                input_dir: in_dir,
                output_dir: out_dir,
                item: None,
                progress: 0.0,
            }]
        },
        Visibility::Visible,
        InheritedVisibility::default(),
        GlobalTransform::default(),
    ))
    // 🏹 2. 添加白色箭头 (作为子实体)
    .with_children(|parent| {
            parent.spawn((
            Transform {
                rotation: rotation_quat, // 应用旋转
                translation: Vec3::new(0.0, 0.0, 0.1), // Z=0.1 浮在表面
                ..default()
            },
            // 必须添加可见性组件，否则子物体（箭头）看不见
            Visibility::Visible,
            InheritedVisibility::default(),
            GlobalTransform::default(), 
        ))
    .with_children(|arrow_parent| {
            // ⚪ 部件 A: 箭杆 (Shaft)
            arrow_parent.spawn(Sprite {
                color: Color::WHITE,
                custom_size: Some(Vec2::new(20.0, 4.0)), // 长条
                ..default()
            });

            // ⚪ 部件 B: 上箭翼 (Upper Wing)
            arrow_parent.spawn((
                Sprite {
                    color: Color::WHITE,
                    custom_size: Some(Vec2::new(10.0, 4.0)),
                    ..default()
                },
                // 位置在右侧，旋转 -45度
                Transform {
                    translation: Vec3::new(5.0, 4.0, 0.0),
                    rotation: Quat::from_rotation_z(-std::f32::consts::FRAC_PI_4),
                    ..default()
                }
            ));

            // ⚪ 部件 C: 下箭翼 (Lower Wing)
            arrow_parent.spawn((
                Sprite {
                    color: Color::WHITE,
                    custom_size: Some(Vec2::new(10.0, 4.0)),
                    ..default()
                },
                // 位置在右侧，旋转 +45度
                Transform {
                    translation: Vec3::new(5.0, -4.0, 0.0),
                    rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_4),
                    ..default()
                }
            ));
             arrow_parent.spawn(Sprite {
                color: Color::WHITE,
                custom_size: Some(Vec2::new(20.0, 4.0)), 
                ..default()
            });
            // ...
        });
    })
    .id();
    map.entities.insert(pos, belt_ent);
}

// 辅助：生成机器
// 修改后的 spawn_machine
fn spawn_machine(
    commands: &mut Commands, 
    map: &mut GridMap, 
    proto_lib: &PrototypeLibrary, // <--- 新增参数：我们需要查阅图纸
    pos: IVec2, 
    proto_id: String
) {
    let proto = proto_lib.machines.get(&proto_id).unwrap();
    let machine_size = IVec2::new(proto.width as i32, proto.length as i32);

    // 1. 生成机器本身 (父实体)
    let machine_ent = commands.spawn((
        Sprite {
            color: Color::srgb(0.2, 0.2, 0.8), // 蓝色底座
            // 稍微留一点缝隙，别填满格子
            custom_size: Some(machine_size.as_vec2() * TILE_SIZE - 2.0), 
            ..default()
        },
        // 机器的中心点位置
        // 注意：Z=0.1 确保它压在传送带上面
        Transform::from_translation(((pos.as_vec2() + Vec2::new(1.0, 1.0)) * TILE_SIZE).extend(0.1)),
        GridPos(pos),
        Machine {
            prototype_id: proto_id.clone(),
            direction: Direction::North, // 初始默认朝北
            progress: 0.0,
            state: MachineState::Idle,
            input_buffer: HashMap::new(),
            output_buffer: HashMap::new(),
            next_output_idx: 0,
            current_recipe_idx: 0,
        },
        // 确保它可见
        Visibility::Visible,
        InheritedVisibility::default(),
        GlobalTransform::default(),
    ))
    // 2. 添加子实体 (指示器)
    .with_children(|parent| {
        // 遍历 Layout，寻找端口
        for y in 0..proto.length {
            for x in 0..proto.width {
                let port_type = proto.layout[y as usize][x as usize];
                
                if port_type != PortType::None {
                    // --- 计算局部偏移 (Local Offset) ---
                    // 机器的中心是 (0,0)
                    // 我们需要把格子的 (x,y) 映射到相对于中心的坐标
                    // 比如 3x3 机器: 
                    // 左下角 (0,0) -> 局部 (-40, -40)
                    // 中心   (1,1) -> 局部 (0, 0)
                    // 右上角 (2,2) -> 局部 (40, 40)
                    
                    let center_offset_x = (proto.width as f32 - 1.0) / 2.0;
                    let center_offset_y = (proto.length as f32 - 1.0) / 2.0;
                    
                    let local_x = (x as f32 - center_offset_x) * TILE_SIZE;
                    let local_y = (y as f32 - center_offset_y) * TILE_SIZE;

                    // 确定颜色和形状
                    let (color, size) = match port_type {
                        PortType::Input => (Color::srgb(0.0, 1.0, 0.0), Vec2::new(30.0, 10.0)), // 绿色宽条
                        PortType::Output => (Color::srgb(1.0, 0.0, 0.0), Vec2::new(30.0, 10.0)), // 红色宽条
                        _ => (Color::WHITE, Vec2::ZERO),
                    };

                    // 生成指示器 Sprite
                    parent.spawn((
                        Sprite {
                            color,
                            custom_size: Some(size),
                            ..default()
                        },
                        // Z=0.1 确保显示在机器底座上方
                        Transform::from_xyz(local_x, local_y, 0.1),
                    ));
                    
                    // 可选：如果你想要更像箭头的效果，可以用 Triangle Mesh，
                    // 但最简单的方法是用长方形条表示端口位置，或者加载一个箭头图片。
                    // 这里为了纯代码实现，我们用长方形条。
                }
            }
        }
    })
    .id();
    
    // 注册到 GridMap
    // (简单的 3x3 占位注册)
    for y in 0..proto.length {
        for x in 0..proto.width {
            let offset = IVec2::new(x as i32, y as i32);
            map.entities.insert(pos + offset, machine_ent);
        }
    }
}

// --- 核心逻辑系统 ---

// 3. 传送带内部移动 (Backpressure 实现)
fn tick_belts_movement(
    time: Res<Time>,
    mut belts: Query<&mut ConveyorBelt>,
) {
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
    // 只需要查询传送带
    mut belt_query: Query<(Entity, &GridPos, &mut ConveyorBelt)>,
    map: Res<GridMap>,
) {
    // Phase 1: 收集传输请求
    let mut transfers = Vec::new();
    for (entity, pos, belt) in belt_query.iter() {
        for (idx, path) in belt.paths.iter().enumerate() {
            if path.item.is_some() && path.progress >= 1.0 {
                let target_pos = pos.0 + path.output_dir.to_ivec2();
                // 只有当目标位置有实体时才记录
                if let Some(&target_ent) = map.entities.get(&target_pos) {
                    transfers.push((entity, target_ent, idx));
                }
            }
        }
    }

    // Phase 2: 执行传输
    for (src_ent, target_ent, src_idx) in transfers {
        // 排除自环
        if src_ent == target_ent { continue; }

        // 同时获取两个传送带的可变引用
        if let Ok([mut src, mut target]) = belt_query.get_many_mut([src_ent, target_ent]) {
            let src_belt = &src.2;
            // 再次检查源物品是否存在 (可能被前面的逻辑处理了)
            if let Some(item_ent) = src_belt.paths[src_idx].item {
                let out_dir = src_belt.paths[src_idx].output_dir;

                // 检查目标传送带 (假设只有一条路径)
                let target_belt = &mut target.2;
                if let Some(target_path) = target_belt.paths.first_mut() {
                    // 逻辑：目标是空的 + 方向匹配
                    // (target_path.input_dir 指的是它接收的方向，out_dir 是来源方向，两者应该是"同向"流动的)
                    // Bevy 坐标系下，如果 A(North)->B，则 A.out=North。
                    // B 接收从南边来的货，所以 B.in 应该是 North (代表它向北流) 或者 South (代表它接收来自南边的货)?
                    // *修正*: 这里沿用你旧代码的逻辑: target.input == src.output.opposite()
                    // 假设 input_dir 定义为 "Facing" (朝向)，那么面对面就是 opposite。
                    if target_path.item.is_none() && target_path.input_dir == out_dir.opposite() {
                        // 转移
                        target_path.item = Some(item_ent);
                        target_path.progress = 0.0;
                        src.2.paths[src_idx].item = None;
                    }
                }
            }
        }
    }
}

fn tick_belt_to_machine(
    mut commands: Commands,
    // 两个查询分开，互不干扰
    mut belt_query: Query<(&GridPos, &mut ConveyorBelt)>,
    mut machine_query: Query<(&GridPos, &mut Machine)>,
    // 物品查询 (用于配方检查)
    item_query: Query<&Item>,
    map: Res<GridMap>,
    proto_lib: Res<PrototypeLibrary>,
) {
    // 直接遍历传送带 (因为不需要同时借用两个 Belt，所以直接 iter_mut 是安全的)
    for (belt_pos, mut belt) in belt_query.iter_mut() {
        for path in belt.paths.iter_mut() {
            // 1. 检查是否有物品待传输
            if let Some(item_ent) = path.item {
                if path.progress >= 1.0 {
                    let target_pos = belt_pos.0 + path.output_dir.to_ivec2();
                    let belt_out_dir = path.output_dir;

                    // 2. 检查目标是否是机器
                    if let Some(target_ent) = map.entities.get(&target_pos) {
                        if let Ok((m_pos, mut machine)) = machine_query.get_mut(*target_ent) {
                            if let Some(proto) = proto_lib.machines.get(&machine.prototype_id) {
                                
                                // --- A. 端口位置与方向检查 ---
                                let input_ports = proto.get_ports_with_facing(m_pos.0, machine.direction, PortType::Input);
                                
                                let mut is_valid = false;
                                for (port_pos, port_facing) in input_ports {
                                    // 坐标匹配 && 方向对冲 (Belt流出方向 == 端口朝外方向的反向)
                                    if port_pos == target_pos && belt_out_dir == port_facing.opposite() {
                                        is_valid = true;
                                        break;
                                    }
                                }

                                if !is_valid { continue; }

                                // --- B. 配方与容量检查 ---
                                if let Ok(item_cmp) = item_query.get(item_ent) {
                                    if let Some(recipe) = proto.recipes.get(machine.current_recipe_idx) {
                                        // 检查物品是否在配方需求中
                                        if recipe.inputs.iter().any(|req| req.item == item_cmp.item_type) {
                                            let current_count = machine.input_buffer.get(&item_cmp.item_type).copied().unwrap_or(0);
                                            
                                            // 简单的堆叠限制
                                            if current_count < 50 {
                                                // ✅ 成功接收
                                                machine.input_buffer.insert(item_cmp.item_type.clone(), current_count + 1);
                                                
                                                // 销毁实体
                                                commands.entity(item_ent).despawn();
                                                
                                                // 清空传送带
                                                path.item = None;
                                                // path.progress = 0.0; // 既然没了就不需要重置进度了，或者重置为0也可以
                                            }
                                        }else {
                                            // 👇 新增调试日志：如果不匹配，打印出来
                                            // 只有当距离足够近试图传输时才打印，防止刷屏
                                            if path.progress >= 1.0 {
                                                info!("拒绝接收: 机器配方需求 {:?}, 但传送带物品是 '{}'", 
                                                    recipe.inputs.iter().map(|r| &r.item).collect::<Vec<_>>(), 
                                                    item_cmp.item_type
                                                );
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
    }
}
// 5. 机器生产逻辑
fn tick_machines_process(
    time: Res<Time<Fixed>>,
    mut query: Query<(Entity, &mut Machine)>, // 加上 Entity 方便打印日志
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
                        for input_req in &recipe.inputs {
                            let current_count = machine.input_buffer.get(&input_req.item).copied().unwrap_or(0);
                            if current_count < input_req.count {
                                can_craft = false;
                                break;
                            }
                        }

                        // --- 2. 扣除原料 (安全版) ---
                        if can_craft {
                            for input_req in &recipe.inputs {
                                // 👇 关键修复：不要用 unwrap()，使用 if let Some
                                if let Some(current) = machine.input_buffer.get_mut(&input_req.item) {
                                    if *current >= input_req.count {
                                        *current -= input_req.count;
                                    } else {
                                        // 理论上不会发生，但防止崩溃
                                        error!("逻辑错误: 机器 {:?} 原料 {} 显示足够但扣除时不足！", entity, input_req.item);
                                    }
                                } else {
                                    // 理论上不会发生
                                    error!("逻辑错误: 机器 {:?} 缺少原料 Key: {}", entity, input_req.item);
                                }
                            }
                            
                            machine.state = MachineState::Working;
                            machine.progress = 0.0;
                        }
                    },
                    
                    MachineState::Working => {
                        machine.progress += dt;
                        
                        if machine.progress >= recipe.time {
                            // --- 3. 产出完成 ---
                            for output_prod in &recipe.outputs {
                                let current = machine.output_buffer.get(&output_prod.item).copied().unwrap_or(0);
                                machine.output_buffer.insert(output_prod.item.clone(), current + output_prod.count);
                            }

                            // 检查堆积
                            let is_output_full = machine.output_buffer.values().any(|&count| count >= 50);
                            if is_output_full {
                                machine.state = MachineState::OutputFull;
                            } else {
                                machine.state = MachineState::Idle;
                                machine.progress = 0.0;
                            }
                        }
                    },
                    
                    MachineState::OutputFull => {
                        // 等待输出被拿走
                        let is_output_full = machine.output_buffer.values().any(|&count| count >= 50);
                        if !is_output_full {
                            machine.state = MachineState::Idle;
                            machine.progress = 0.0;
                        }
                    }
                }
            }
        }
    }
}

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
                    
                    // --- 修复核心：计算矢量偏移 ---
                    
                    // 1. 计算起点偏移 (Progress = 0.0)
                    // 如果 Input 是 North，说明物品从北边进来，起点在格子顶部 (0, 0.5)
                    let start_offset = path.input_dir.to_ivec2().as_vec2() * 0.5 * TILE_SIZE;
                    
                    // 2. 计算终点偏移 (Progress = 1.0)
                    // 如果 Output 是 South，说明物品要往南边出去，终点在格子底部 (0, -0.5)
                    let end_offset = path.output_dir.to_ivec2().as_vec2() * 0.5 * TILE_SIZE;
                    
                    // 3. 线性插值 (Lerp)
                    // 这样物品就会沿着正确的方向（比如从上到下）移动
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
    mut machine_query: Query<(&GridPos, &mut Machine)>,
    mut belt_query: Query<&mut ConveyorBelt>,
    map: Res<GridMap>,
    proto_lib: Res<PrototypeLibrary>,
) {
    for (m_pos, mut machine) in machine_query.iter_mut() {
        if let Some(proto) = proto_lib.machines.get(&machine.prototype_id) {
            
            // 获取第一个非空的产物
            // 👇 关键修复：分开获取 Key 和 Value，避免 borrow checker 冲突且不使用 unwrap
            let target_item = machine.output_buffer.iter()
                .find(|(_, &count)| count > 0)
                .map(|(k, &c)| (k.clone(), c)); // Clone key and copy count

            if let Some((item_id, count)) = target_item {
                let output_ports = proto.get_ports_with_facing(m_pos.0, machine.direction, PortType::Output);

                for (out_pos, _out_dir) in output_ports {
                    if let Some(ent) = map.entities.get(&out_pos) {
                        if let Ok(mut belt) = belt_query.get_mut(*ent) {
                            // 简单的传送带检查
                            if let Some(path) = belt.paths.first_mut() {
                                if path.item.is_none() && path.progress < 0.1 {
                                    
                                    // 生成实体
                                    let color = proto_lib.items.get(&item_id).map(|i| i.color).unwrap_or(Color::WHITE);
                                    let item_ent = commands.spawn((
                                        Sprite {
                                            color,
                                            custom_size: Some(Vec2::new(20.0, 20.0)),
                                            ..default()
                                        },
                                        Transform::from_xyz(0.0, 0.0, 1.0),
                                        Item { item_type: item_id.clone(), visual_progress: 0.0 },
                                    )).id();

                                    // 放入传送带
                                    path.item = Some(item_ent);
                                    path.progress = 0.5;

                                    // 扣除库存
                                    // 👇 这里用 unwrap 是安全的，因为上面刚刚 find 过，但也可用 if let
                                    if let Some(c) = machine.output_buffer.get_mut(&item_id) {
                                        *c -= 1;
                                    }
                                    break; 
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}