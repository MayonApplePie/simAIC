use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};

// --- 常量配置 ---
const TILE_SIZE: f32 = 40.0;
const SIM_TICK_RATE: f64 = 20.0; // 逻辑帧率 20 TPS
const BELT_SPEED: f32 = 0.5; // items per second
const BUFFER_CAPACITY: u32 = 50; // 机器缓存上限

// --- 基础枚举与类型 ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Component)]
enum Direction {
    North,
    East,
    South,
    West,
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

// 机器原型数据 (只读)
#[derive(Resource, Default)]
struct PrototypeLibrary {
    machines: HashMap<String, MachinePrototype>,
}

struct MachinePrototype {
    id: String,
    width: u32,
    height: u32,
    layout: Vec<Vec<PortType>>, // 原始布局 [y][x]
    base_time: f32, // 生产耗时
    // 简化：为了 Demo，这里硬编码一个简单的配方
    input_req: String, 
    output_prod: String,
}

// ----------------------------------------------------------------------
// 🔧 核心算法：局部坐标 -> 世界坐标旋转变换
// ----------------------------------------------------------------------

impl MachinePrototype {
    // 获取旋转后的端口列表，返回 [(世界坐标, 端口朝向)]
    // 端口朝向定义：端口"看着"的方向。例如 Input 在顶部，朝向就是 North。
    fn get_ports_with_facing(
        &self, 
        origin: IVec2,         // 机器原点 (GridPos)
        machine_dir: Direction,// 机器当前的旋转
        port_type: PortType    // Input 或 Output
    ) -> Vec<(IVec2, Direction)> {
        
        let mut result = Vec::new();
        
        // 1. 确定当前旋转下的尺寸 (用于坐标变换)
        // 0/180度保持原样，90/270度宽高互换
        let (effective_w, effective_h) = match machine_dir {
            Direction::North | Direction::South => (self.width, self.height),
            Direction::East | Direction::West => (self.height, self.width),
        };

        for y in 0..self.height {
            for x in 0..self.width {
                // 找到对应类型的端口
                if self.layout[y as usize][x as usize] == port_type {
                    
                    // A. 计算局部原始朝向 (假设没旋转时)
                    // 规则：位于边缘的端口朝向外部
                    let local_facing = if y as u32 == self.height - 1 { Direction::North }
                                  else if y == 0 { Direction::South }
                                  else if x == 0 { Direction::West }
                                  else if x as u32 == self.width - 1 { Direction::East }
                                  else { Direction::North }; // 默认 fallback

                    // B. 计算旋转后的朝向
                    let world_facing = match machine_dir {
                        Direction::North => local_facing,
                        Direction::East  => local_facing.rotate_clockwise(),
                        Direction::South => local_facing.opposite(),
                        Direction::West  => local_facing.rotate_counter_clockwise(),
                    };

                    // C. 计算旋转后的坐标偏移 (相对于 origin)
                    // 假设局部原点 (0,0) 在左下角
                    let (rot_x, rot_y) = match machine_dir {
                        Direction::North => (x as i32, y as i32),
                        // 顺时针 90: (x, y) -> (y, W-1-x)  <-- 注意这里 W 是原始宽度
                        Direction::East  => (y as i32, self.width as i32 - 1 - x as i32),
                        // 180: (x, y) -> (W-1-x, H-1-y)
                        Direction::South => (self.width as i32 - 1 - x as i32, self.height as i32 - 1 - y as i32),
                        // 270: (x, y) -> (H-1-y, x)
                        Direction::West  => (self.height as i32 - 1 - y as i32, x as i32),
                    };

                    result.push((origin + IVec2::new(rot_x, rot_y), world_facing));
                }
            }
        }
        result
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
        
        .add_systems(Startup, (setup_prototypes, setup_world).chain())
        
        // 核心仿真循环 (FixedUpdate)
        .add_systems(FixedUpdate, (
            tick_belts_movement,      // 1. 传送带内部移动
            tick_transport_handshake, // 2. 传送带/机器交互 (拓扑传输)
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

// 1. 初始化原型数据 (模拟从文件加载)
fn setup_prototypes(mut lib: ResMut<PrototypeLibrary>) {
    // 定义一个 3x3 的精炼炉: 上进下出
    let layout_str = vec![
        vec![PortType::Output, PortType::Output, PortType::Output], // y=0 (底部)
        vec![PortType::None,   PortType::None,   PortType::None],   // y=1
        vec![PortType::Input,  PortType::Input,  PortType::Input],  // y=2 (顶部)
    ];
    
    lib.machines.insert("refining_unit".to_string(), MachinePrototype {
        id: "refining_unit".to_string(),
        width: 3,
        height: 3,
        layout: layout_str,
        base_time: 2.0,
        input_req: "Ore".to_string(),
        output_prod: "Iron".to_string(),
    });
}

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
        Item { item_type: "Ore".to_string(), visual_progress: 0.0 },
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
    let machine_size = IVec2::new(proto.width as i32, proto.height as i32);

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
        },
        // 确保它可见
        Visibility::Visible,
        InheritedVisibility::default(),
        GlobalTransform::default(),
    ))
    // 2. 添加子实体 (指示器)
    .with_children(|parent| {
        // 遍历 Layout，寻找端口
        for y in 0..proto.height {
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
                    let center_offset_y = (proto.height as f32 - 1.0) / 2.0;
                    
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
    for y in 0..proto.height {
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
fn tick_transport_handshake(
    mut commands: Commands,
    map: Res<GridMap>,
    proto_lib: Res<PrototypeLibrary>,
    mut belt_query: Query<(Entity, &GridPos, &mut ConveyorBelt)>,
    mut machine_query: Query<(Entity, &GridPos, &mut Machine)>,
) {
    let mut transfers: Vec<(Entity, IVec2, usize)> = Vec::new();

    // 1. 收集传输请求
    for (entity, pos, belt) in belt_query.iter() {
        for (idx, path) in belt.paths.iter().enumerate() {
            if path.item.is_some() && path.progress >= 1.0 {
                let target_pos = pos.0 + path.output_dir.to_ivec2();
                transfers.push((entity, target_pos, idx));
            }
        }
    }

    // 2. 执行传输
    for (src_entity, target_pos, src_path_idx) in transfers {
        
        // 预先检查：源物品是否存在（避免之后的复杂逻辑扑空）
        // 这里只获取 item entity id，不需要借用 component
        let item_entity_opt = if let Ok((_, _, belt)) = belt_query.get(src_entity) {
             belt.paths[src_path_idx].item
        } else { None };

        if item_entity_opt.is_none() { continue; }
        let item_entity = item_entity_opt.unwrap();

        if let Some(&target_ent) = map.entities.get(&target_pos) {
            
            // --- 情况 I: 目标是传送带 (使用 get_many_mut 解决借用冲突) ---
            if belt_query.contains(target_ent) {
                // 确保源和目标不是同一个实体（虽然逻辑上不太可能，但 Rust 需要保证）
                if src_entity != target_ent {
                    if let Ok([mut src_entry, mut target_entry]) = belt_query.get_many_mut([src_entity, target_ent]) {
                        // 解构出我们需要的组件
                        // get_many_mut 返回的是数组，顺序对应输入的 entity 顺序
                        let src_belt = &src_entry.2; // 先只读引用，获取方向
                        let belt_out_dir = src_belt.paths[src_path_idx].output_dir;
                        
                        // 获取目标传送带的可变引用
                        let target_belt = &mut target_entry.2;
                        let target_path = &mut target_belt.paths[0]; // 假设第一条路径

                        // 核心判定
                        if target_path.item.is_none() && target_path.input_dir == belt_out_dir.opposite() {
                            // 1. 转移物品所有权
                            target_path.item = Some(item_entity);
                            target_path.progress = 0.0;
                            
                            // 2. 清除源头 (因为 src_entry 也是 Mut 的，所以可以直接改)
                            src_entry.2.paths[src_path_idx].item = None;
                        }
                    }
                }
            }
            // --- 情况 II: 目标是机器 ---
            else if let Ok((_, machine_pos, mut machine)) = machine_query.get_mut(target_ent) {
                let proto = proto_lib.machines.get(&machine.prototype_id).unwrap();
                
                // 1. 获取源头流向
                // 【关键修复】这里我们只读取 Direction (它是一个 Copy 类型)
                // 读取完立刻结束借用，这样就不会和后面的 get_mut 冲突
                let belt_out_dir = if let Ok((_, _, src_belt)) = belt_query.get(src_entity) {
                    src_belt.paths[src_path_idx].output_dir
                } else {
                    continue; // 源头如果拿不到，就跳过
                };

                // 2. 获取机器端口
                let input_ports = proto.get_ports_with_facing(machine_pos.0, machine.direction, PortType::Input);
                
                let mut valid_port = false;

                // 3. 遍历端口寻找匹配
                for (port_pos, port_facing) in input_ports {
                    if port_pos == target_pos {
                        if belt_out_dir == port_facing.opposite() {
                            valid_port = true;
                            break; 
                        }
                    }
                }

                if valid_port {
                    let item_type = "Ore".to_string(); 
                    let count = machine.input_buffer.get(&item_type).copied().unwrap_or(0);
                    
                    if count < BUFFER_CAPACITY {
                        machine.input_buffer.insert(item_type, count + 1);
                        
                        // 清除源头 (此时 machine_query 的借用已经结束，可以安全地再次借用 belt_query)
                        if let Ok((_, _, mut src_belt)) = belt_query.get_mut(src_entity) {
                            src_belt.paths[src_path_idx].item = None;
                        }
                        
                        commands.entity(item_entity).despawn();
                        println!("机器严格接收物品成功！库存: {}", count + 1);
                    }
                }
            }
        }
    }
}
// 5. 机器生产逻辑
fn tick_machines_process(
    time: Res<Time>,
    proto_lib: Res<PrototypeLibrary>,
    mut machines: Query<&mut Machine>,
) {
    let dt = time.delta_secs();

    for mut machine in machines.iter_mut() {
        let proto = proto_lib.machines.get(&machine.prototype_id).unwrap();
        
        // 简化配方：1 Ore -> 1 Iron
        let input_key = &proto.input_req;
        let output_key = &proto.output_prod;

        match machine.state {
            MachineState::Idle => {
                let input_count = machine.input_buffer.get(input_key).copied().unwrap_or(0);
                if input_count >= 1 {
                    // 开始生产
                    machine.input_buffer.insert(input_key.clone(), input_count - 1);
                    machine.state = MachineState::Working;
                    machine.progress = 0.0;
                }
            },
            MachineState::Working => {
                machine.progress += dt;
                if machine.progress >= proto.base_time {
                    // 生产完成，尝试放入输出区
                    let output_count = machine.output_buffer.get(output_key).copied().unwrap_or(0);
                    if output_count < BUFFER_CAPACITY {
                        machine.output_buffer.insert(output_key.clone(), output_count + 1);
                        machine.state = MachineState::Idle; // 回到空闲
                        println!("生产完成！产物库存: {}", output_count + 1);
                    } else {
                        machine.state = MachineState::OutputFull; // 堵塞
                    }
                }
            },
            MachineState::OutputFull => {
                // 检查是否有空位了
                let output_count = machine.output_buffer.get(output_key).copied().unwrap_or(0);
                if output_count < BUFFER_CAPACITY {
                    machine.state = MachineState::Working; // 恢复工作（实际上是恢复放入逻辑）
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
    mut machines: Query<(&GridPos, &mut Machine)>,
    // 注意：Query 返回的是 (Entity, &GridPos, &mut ConveyorBelt)
    mut belts: Query<(Entity, &GridPos, &mut ConveyorBelt)>, 
    map: Res<GridMap>,
    proto_lib: Res<PrototypeLibrary>,
) {
    for (pos, mut machine) in machines.iter_mut() {
        let proto = proto_lib.machines.get(&machine.prototype_id).unwrap();
        let output_key = &proto.output_prod;

        let count = machine.output_buffer.get(output_key).copied().unwrap_or(0);
        if count > 0 {
            // 1. 获取所有 Output 端口
            let output_ports = proto.get_ports_with_facing(pos.0, machine.direction, PortType::Output);
            
            if output_ports.is_empty() { continue; }

            let idx = machine.next_output_idx % output_ports.len();
            let (port_pos, port_facing) = output_ports[idx];

            // 2. 喷射方向 = 端口朝向
            let eject_dir = port_facing; 
            let target_pos = port_pos + eject_dir.to_ivec2();

            // 3. 检查目标
            if let Some(target_ent) = map.entities.get(&target_pos) {
                // 修正点：使用 get_mut 并解构 ((_, _, mut target_belt))
                if let Ok((_, _, mut target_belt)) = belts.get_mut(*target_ent) {
                    
                    let target_path = &mut target_belt.paths[0];
                    
                    // 4. 严格方向检查：传送带 Input 必须等于 喷射方向.opposite()
                    // 比如机器向南(South)喷射，传送带必须接受来自北(North)的物品 (即 Input=North)
                    // 而 North == South.opposite()
                    if target_path.input_dir == eject_dir.opposite() {
                        
                        if target_path.item.is_none() && target_path.progress < 0.1 {
                            
                            let item_ent = commands.spawn((
                                Sprite {
                                    color: Color::srgb(0.5, 0.5, 1.0),
                                    custom_size: Some(Vec2::new(20.0, 20.0)),
                                    ..default()
                                },
                                Transform::from_translation((port_pos.as_vec2() * TILE_SIZE).extend(1.0)), 
                                Item { 
                                    item_type: output_key.clone(), 
                                    visual_progress: 0.0 
                                },
                            )).id();

                            target_path.item = Some(item_ent);
                            target_path.progress = 0.0; 
                            
                            machine.output_buffer.insert(output_key.clone(), count - 1);
                            println!("机器严格输出成功 -> {:?}", target_pos);

                            machine.next_output_idx = (machine.next_output_idx + 1) % output_ports.len();
                        }
                    }
                }
            }
        }
    }
}
