use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};

use bevy::color::Srgba;
use serde::Deserialize;
use std::fs;

// --- 常量配置 ---
const TILE_SIZE: f32 = 40.0;
const SIM_TICK_RATE: f64 = 60.0; // 逻辑帧率 20 TPS
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
        
        .add_systems(Startup, (load_config, setup_scenario).chain())
        
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
                    m.dir // 👈 读取 TOML 里的方向
                );
            } else {
                error!("❌ Scenario 引用了不存在的机器 ID: {}", m.proto_id);
            }
        }
    }

    // 4. 生成传送带
    if let Some(belts) = config.belts {
        for b in belts {
            spawn_belt(&mut commands, &mut map, IVec2::new(b.x, b.y), b.in_dir, b.out_dir);
        }
    }

    // 5. 生成初始物品
    if let Some(items) = config.items {
        for i in items {
            // 获取颜色
            let color = proto_lib.items.get(&i.proto_id)
                .map(|p| p.color)
                .unwrap_or_else(|| {
                    warn!("⚠️ 物品 ID [{}] 未定义，使用默认颜色", i.proto_id);
                    Color::WHITE
                });

            let item_ent = commands.spawn((
                Sprite {
                    color,
                    custom_size: Some(Vec2::new(20.0, 20.0)),
                    ..default()
                },
                Transform::from_xyz(0.0, 0.0, 1.0), 
                Item { 
                    item_type: i.proto_id.clone(), // 👈 这里才是真正使用了 TOML 里的 ID
                    visual_progress: 0.0 
                },
            )).id();

            // 放入传送带
            let pos = IVec2::new(i.x, i.y);
            if let Some(belt_ent) = map.entities.get(&pos) {
                 commands.entity(*belt_ent).entry::<ConveyorBelt>().and_modify(move |mut belt| {
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
        let in_name = recipe.inputs.get(0).map(|i| i.item.clone()).unwrap_or("None".to_string());
        let out_name = recipe.outputs.get(0).map(|i| i.item.clone()).unwrap_or("None".to_string());
        format!("In: {}\nOut: {}", in_name, out_name)
    } else { "No Recipe".to_string() };
    
    // 3. 生成实体
    let machine_ent = commands.spawn((
        Sprite {
            color: Color::srgb(0.2, 0.2, 0.8),
            // 注意：这里使用原始宽高的 Box 即可，因为父级 Transform 会旋转它
            custom_size: Some(Vec2::new(proto.width as f32, proto.length as f32) * TILE_SIZE - 2.0), 
            ..default()
        },
        Transform {
            // 计算中心点偏移：(W/2, L/2)
            // 无论旋转与否，Sprite 都是基于自身局部坐标系的，所以用原始宽高
            translation: ((pos.as_vec2() + Vec2::new(proto.width as f32 / 2.0, proto.length as f32 / 2.0)) * TILE_SIZE - Vec2::splat(TILE_SIZE * 0.5)).extend(0.1),
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
                        Sprite { color, custom_size: Some(size), ..default() },
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
        if src_ent == target_ent { continue; }

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
                    let belt_out_dir = path.output_dir;

                    // 1. 检查目标位置是否有实体
                    if let Some(target_ent) = map.entities.get(&target_pos) {
                        // 2. 检查目标是否是机器
                        if let Ok((m_pos, mut machine)) = machine_query.get_mut(*target_ent) {
                            if let Some(proto) = proto_lib.machines.get(&machine.prototype_id) {
                                
                                // --- Debug: 找到了机器，开始检查端口 ---
                                let input_ports = proto.get_ports_with_facing(m_pos.0, machine.direction, PortType::Input);
                                
                                let mut is_port_valid = false;
                                for (port_pos, port_facing) in &input_ports {
                                    // 坐标匹配 && 方向对冲 (Belt流出方向 == 端口朝外方向的反向)
                                    if *port_pos == target_pos && belt_out_dir == port_facing.opposite() {
                                        is_port_valid = true;
                                        break;
                                    }
                                }

                                if !is_port_valid {
                                    // ⚠️ 失败原因 1: 端口不对
                                    // 防止刷屏，只在特定的 tick 打印，或者你可以暂时忍受刷屏
                                    info!("⛔ 拒绝接收 [端口错误]: 传送带在 {:?} 向 {:?} 输出，但机器 {:?} 的入口位于 {:?} (朝向 {:?})", 
                                        target_pos, belt_out_dir, machine.prototype_id, 
                                        input_ports.iter().map(|(p, _)| p).collect::<Vec<_>>(),
                                        input_ports.iter().map(|(_, d)| d).collect::<Vec<_>>()
                                    );
                                    continue; 
                                }

                                // --- Debug: 端口正确，开始检查配方 ---
                                if let Ok(item_cmp) = item_query.get(item_ent) {
                                    // 获取当前配方
                                    if let Some(recipe) = proto.recipes.get(machine.current_recipe_idx) {
                                        
                                        // 检查物品是否在配方需求中
                                        let is_needed = recipe.inputs.iter().any(|req| req.item == item_cmp.item_type);

                                        if is_needed {
                                            let current_count = machine.input_buffer.get(&item_cmp.item_type).copied().unwrap_or(0);
                                            
                                            // 检查库存容量
                                            if current_count < BUFFER_CAPACITY {
                                                // ✅ 成功接收 (这是原来的逻辑)
                                                machine.input_buffer.insert(item_cmp.item_type.clone(), current_count + 1);
                                                commands.entity(item_ent).despawn();
                                                path.item = None;
                                                info!("✅ 成功接收: {}", item_cmp.item_type);
                                            } else {
                                                // ⚠️ 失败原因 3: 库存已满
                                                info!("⛔ 拒绝接收 [库存已满]: {} 库存: {}", item_cmp.item_type, current_count);
                                            }
                                        } else {
                                            // ⚠️ 失败原因 2: 配方不匹配
                                            // 打印出机器当前想要什么，以及传送带上是什么
                                            let wanted: Vec<String> = recipe.inputs.iter().map(|r| r.item.clone()).collect();
                                            info!("⛔ 拒绝接收 [配方不配]: 机器需要 {:?}, 但传送带物品是 '{}'", wanted, item_cmp.item_type);
                                        }
                                    } else {
                                        info!("⛔ 拒绝接收 [无配方]: 机器当前没有设置配方 (idx={})", machine.current_recipe_idx);
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
                            let current_count = machine.input_buffer.get(&input_req.item).copied().unwrap_or(0);
                            
                            if current_count < input_req.count {
                                can_craft = false;
                                // 记录缺料详情
                                missing_info = format!("{} (持有: {}, 需要: {})", input_req.item, current_count, input_req.count);
                                
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
                                if let Some(current) = machine.input_buffer.get_mut(&input_req.item) {
                                    if *current >= input_req.count {
                                        *current -= input_req.count;
                                    } else {
                                        error!("❌ 逻辑错误: 机器 {:?} 原料 {} 检查通过但扣除失败！", entity, input_req.item);
                                    }
                                } else {
                                    error!("❌ 逻辑错误: 机器 {:?} 缺少原料 Key: {}", entity, input_req.item);
                                }
                            }
                            
                            machine.state = MachineState::Working;
                            machine.progress = 0.0;
                            
                            // ✅ 打印开始生产日志
                            info!("⚙️ 机器 [{:?}] 开始生产 (配方耗时: {:.1}s)", entity, recipe.time);
                        }
                    },
                    
                    MachineState::Working => {
                        machine.progress += dt;
                        
                        // (可选) 打印进度调试，如果配方时间很长可以解开下面注释
                        // if machine.progress % 1.0 < dt { info!("...生产中 {:.1}s / {:.1}s", machine.progress, recipe.time); }
                        
                        if machine.progress >= recipe.time {
                            // --- 3. 产出完成 ---
                            for output_prod in &recipe.outputs {
                                let current = machine.output_buffer.get(&output_prod.item).copied().unwrap_or(0);
                                machine.output_buffer.insert(output_prod.item.clone(), current + output_prod.count);
                                
                                // ✅ 打印产出日志
                                info!("✨ 机器 [{:?}] 生产完成! 产出: {} (+{}) | 当前库存: {}", 
                                    entity, output_prod.item, output_prod.count, current + output_prod.count);
                            }

                            // 检查堆积 (这里硬编码了 50 作为上限，之后可以改为常量配置)
                            let is_output_full = machine.output_buffer.values().any(|&count| count >= 50);
                            
                            if is_output_full {
                                machine.state = MachineState::OutputFull;
                                warn!("⚠️ 机器 [{:?}] 出口堵塞! 产物堆积已满，停止工作。", entity);
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
                            info!("♻️ 机器 [{:?}] 堵塞解除，恢复工作。", entity);
                        }
                    }
                }
            } else {
                // 如果配方索引越界
                warn!("❌ 机器 [{:?}] 配方索引错误: {}", entity, machine.current_recipe_idx);
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
    mut machine_query: Query<(Entity, &GridPos, &mut Machine)>,
    // 优化：不再需要遍历所有传送带，只需要通过 Entity 查询特定的一个
    mut belt_query: Query<&mut ConveyorBelt>, 
    map: Res<GridMap>,
    proto_lib: Res<PrototypeLibrary>,
) {
    for (entity, m_pos, mut machine) in machine_query.iter_mut() {
        if let Some(proto) = proto_lib.machines.get(&machine.prototype_id) {
            
            // 1. 检查有没有产物需要输出
            // 使用迭代器找到第一个非空产物
            let item_to_output = machine.output_buffer.iter()
                .find(|(_, &count)| count > 0)
                .map(|(k, &c)| (k.clone(), c));

            if let Some((item_id, count)) = item_to_output {
                
                // 2. 获取端口信息
                let output_ports = proto.get_ports_with_facing(m_pos.0, machine.direction, PortType::Output);
                let mut success = false;

                for (port_pos, port_dir) in output_ports {
                    
                    // 3. 计算喷射目标位置 (机器外部的一格)
                    let target_pos = port_pos + port_dir.to_ivec2();

                    // --- 🚀 性能优化: 使用 GridMap 直接查找 ---
                    // 不再遍历所有传送带，直接问地图：target_pos 有谁？
                    if let Some(&target_ent) = map.entities.get(&target_pos) {
                        
                        // 4. 检查这个实体是不是传送带
                        if let Ok(mut belt) = belt_query.get_mut(target_ent) {
                            
                            if let Some(path) = belt.paths.first_mut() {
                                
                                // 🔥🔥🔥 关键修复: 只要没物品就可以放！🔥🔥🔥
                                // 删除了 `&& path.progress < 0.1` 的限制
                                // 无论之前的进度是多少，只要现在是空的，我们就重置进度并放入新物品
                                if path.item.is_none() {
                                    
                                    // === A. 生成物品实体 ===
                                    let color = proto_lib.items.get(&item_id).map(|i| i.color).unwrap_or(Color::WHITE);
                                    let item_ent = commands.spawn((
                                        Sprite {
                                            color,
                                            custom_size: Some(Vec2::new(20.0, 20.0)),
                                            ..default()
                                        },
                                        // 初始位置设为端口位置，稍微好看点（之后 visuals 会同步）
                                        Transform::from_translation((port_pos.as_vec2() * TILE_SIZE).extend(1.0)),
                                        Item { item_type: item_id.clone(), visual_progress: 0.0 },
                                    )).id();

                                    // === B. 放入传送带 ===
                                    path.item = Some(item_ent);
                                    
                                    // ⚡️ 重置进度：这一步至关重要，它覆盖了之前的幽灵进度
                                    path.progress = 0.5; 

                                    // === C. 扣除机器库存 ===
                                    if let Some(c) = machine.output_buffer.get_mut(&item_id) {
                                        *c -= 1;
                                    }
                                    
                                    info!("✅ 机器 [{:?}] 成功喷射: {} -> 位置 {:?}", entity, item_id, target_pos);
                                    success = true;
                                    break; // 成功处理一个就退出端口循环
                                } 
                            }
                        }
                    }
                }
                
                if !success {
                    // 如果尝试了所有端口都失败，说明真的堵了
                    // debug!("⚠️ 机器 [{:?}] 输出受阻", entity);
                }
            }
        }
    }
}