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

impl MachinePrototype {
    // 获取旋转后的所有端口位置
    fn get_ports(&self, origin: IVec2, dir: Direction, p_type: PortType) -> Vec<IVec2> {
        let mut ports = Vec::new();
        let size = IVec2::new(self.width as i32, self.height as i32);
        
        for y in 0..self.height {
            for x in 0..self.width {
                if self.layout[y as usize][x as usize] == p_type {
                    let local = IVec2::new(x as i32, y as i32);
                    let world_offset = dir.rotate_point(local, size);
                    ports.push(origin + world_offset);
                }
            }
        }
        ports
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
        ).chain())
        // .insert_resource(ClearColor(Color::BLACK))
        // 渲染同步 (Update)
        .add_systems(Update, sync_visuals)
        
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
) {
    commands.spawn((
            Camera2d::default(),
            // Z = 999.0 确保相机在所有物体的前面
            // scale = 0.5 意味着放大 2 倍 (数值越小越放大)
            Transform::from_xyz(0.0, 0.0, 300.0).with_scale(Vec3::splat(0.5)), 
        ));
    // A. 放置一个传送带 (0,0) -> (1,0)
    spawn_belt(&mut commands, &mut map, IVec2::new(0, 0), Direction::West, Direction::East);
    spawn_belt(&mut commands, &mut map, IVec2::new(1, 0), Direction::West, Direction::East);
    
    // 在第一个传送带上放一个物品
    let item_ent = commands.spawn((
        Sprite {
            color: Color::srgb(1.0, 0.5, 0.0),
            custom_size: Some(Vec2::new(20.0, 20.0)),
            ..default()
        },
        Transform::default(),
        Item { item_type: "Ore".to_string(), visual_progress: 0.0 },
    )).id();
    
    // 把物品挂载到 (0,0) 传送带的第一个路径上
    if let Some(belt_ent) = map.entities.get(&IVec2::new(0, 0)) {
        // ✅ 修正代码 (添加 move)
        commands.entity(*belt_ent).entry::<ConveyorBelt>().and_modify(move |mut belt| { // <--- 这里加 move
            belt.paths[0].item = Some(item_ent);
            belt.paths[0].progress = 0.5;
        });
    }

    // B. 放置一个机器 (2, -1) -> 这样它的左边 Input 刚好对着 (1,0) 的传送带
    // 3x3 机器，原点在左下角。Input 在 y=2。
    // 如果放置在 (2, -2)，则 y=2 (相对) -> 世界坐标 y=0。
    spawn_machine(&mut commands, &mut map, IVec2::new(2, -2), "refining_unit".to_string());
}

// 辅助：生成传送带
fn spawn_belt(commands: &mut Commands, map: &mut GridMap, pos: IVec2, in_dir: Direction, out_dir: Direction) {
    let ent = commands.spawn((
        Sprite {
            color: Color::srgb(0.3, 0.3, 0.3),
            custom_size: Some(Vec2::new(38.0, 38.0)),
            ..default()
        },
        Transform::from_translation((pos.as_vec2() * TILE_SIZE).extend(0.0)),
        Visibility::Visible, 
        GlobalTransform::default(),
        GridPos(pos),
        ConveyorBelt {
            paths: vec![BeltPath {
                input_dir: in_dir,
                output_dir: out_dir,
                item: None,
                progress: 0.0,
            }]
        }
    )).id();
    map.entities.insert(pos, ent);
}

// 辅助：生成机器
fn spawn_machine(commands: &mut Commands, map: &mut GridMap, pos: IVec2, proto_id: String) {
    let ent = commands.spawn((
        Sprite {
            color: Color::srgb(0.2, 0.2, 0.8), // 蓝色机器
            custom_size: Some(Vec2::new(3.0 * TILE_SIZE - 2.0, 3.0 * TILE_SIZE - 2.0)), // 3x3
            ..default()
        },
        // 3x3 的中心点需要偏移
        Transform::from_translation(((pos.as_vec2() + Vec2::new(1.0, 1.0)) * TILE_SIZE).extend(0.1)),
        GridPos(pos),
        Machine {
            prototype_id: proto_id,
            direction: Direction::North,
            progress: 0.0,
            state: MachineState::Idle,
            input_buffer: HashMap::new(),
            output_buffer: HashMap::new(),
            next_output_idx: 0,
        }
    )).id();
    
    // 占位逻辑：机器占用了 3x3 的格子
    // 实际项目中应该有一个 Footprint 组件来处理，这里简化，只注册原点
    // 注意：这意味着为了 Demo 简单，我们只注册了 (2, -2) 这个点到 GridMap
    // 完整的逻辑应该把 (2,-2) 到 (4,0) 全部注册为指向该机器的引用。
    map.entities.insert(pos, ent); 
    // 为了让 input 端口 (2, 0) 能被找到，我们手动注册一下端口位置
    map.entities.insert(pos + IVec2::new(0, 2), ent);
    map.entities.insert(pos + IVec2::new(1, 2), ent);
    map.entities.insert(pos + IVec2::new(2, 2), ent);
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
    // 为了同时借用两个 Belt，我们需要 unsafe 或者 split query。
    // 这里为了安全和简单，使用 "Gather Requests -> Execute" 模式
    // 但对于 Bevy，使用 Entity 索引直接 get_many_mut 是最高效的。
    mut belt_query: Query<(Entity, &GridPos, &mut ConveyorBelt)>,
    mut machine_query: Query<(Entity, &GridPos, &mut Machine)>,
) {
    // --- A. 传送带 -> 传送带/机器 ---
    // 我们需要收集所有 belt 的传输意图，避免遍历时借用冲突
    // (在真实项目中，这里通常会用 unsafe 来规避，或者将 GridMap 存储为 Component)
    // 为了演示代码简洁，我们采用简单的 "遍历所有 Belt，尝试推给邻居"
    // 注意：这在并行 System 中会有问题，但在单线程逻辑中是可行的。
    
    // 我们先收集“我想传输”的请求，因为不能在遍历 belt_query 时去 get_mut 另一个 belt
    let mut transfers: Vec<(Entity, IVec2, usize)> = Vec::new(); // (SourceBeltEntity, TargetPos, PathIndex)

    for (entity, pos, belt) in belt_query.iter() {
        for (idx, path) in belt.paths.iter().enumerate() {
            if path.item.is_some() && path.progress >= 1.0 {
                let target_pos = pos.0 + path.output_dir.to_ivec2();
                transfers.push((entity, target_pos, idx));
            }
        }
    }

    // 执行传输
    for (src_entity, target_pos, src_path_idx) in transfers {
        
        // 1. 尝试获取 Source 的 Item Entity (只读)
        let item_entity = if let Ok(belt) = belt_query.get(src_entity) {
             belt.2.paths[src_path_idx].item
        } else { None };

        if item_entity.is_none() { continue; }
        let item_entity = item_entity.unwrap();

        // 2. 检查目标是什么
        if let Some(&target_ent) = map.entities.get(&target_pos) {
            
            // 情况 I: 目标是传送带
            if let Ok((_, _, mut target_belt)) = belt_query.get_mut(target_ent) {
                // 寻找匹配的路径: Target Input == Source Output (Opposite)
                // 这里简化：假设 Source Output 是 East，Target 必须接受 West 输入
                let mut success = false;
                
                // 为了解耦 Borrow Checker，这里再次获取 Source Belt 的 Output Direction
                // 这是一个 Hack，实际工程中最好传递 Direction
                let src_out_dir = Direction::East; // 假设 Demo 都是向东
                
                for target_path in target_belt.paths.iter_mut() {
                    // 简单的衔接判定
                    if target_path.input_dir == src_out_dir.opposite() && target_path.item.is_none() {
                         // 成功转移！
                         target_path.item = Some(item_entity);
                         target_path.progress = 0.0;
                         success = true;
                         break;
                    }
                }
                
                if success {
                     // 清除源头
                     let mut src_belt = belt_query.get_mut(src_entity).unwrap().2;
                     src_belt.paths[src_path_idx].item = None;
                }
            }
            // 情况 II: 目标是机器
            else if let Ok((_, _, mut machine)) = machine_query.get_mut(target_ent) {
                // 检查机器是否有端口在这里 (需要 Prototype 数据)
                // 简化：假设只要在 Map 里查到了这个机器，且机器没满，就塞进去
                // 真实的逻辑需要 proto.get_ports() 校验 target_pos 是否是 Input 端口
                
                let item_type = "Ore".to_string(); // 应该从 Item Component 获取
                let count = machine.input_buffer.get(&item_type).copied().unwrap_or(0);
                
                if count < BUFFER_CAPACITY {
                    // 接收
                    machine.input_buffer.insert(item_type, count + 1);
                    
                    // 清除源头
                    let mut src_belt = belt_query.get_mut(src_entity).unwrap().2;
                    src_belt.paths[src_path_idx].item = None;
                    
                    // 销毁物品实体 (进入机器内部)
                    commands.entity(item_entity).despawn();
                    println!("机器接收了物品！当前库存: {}", count + 1);
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
                    // 简单的线性插值
                    let base_pos = pos.0.as_vec2() * TILE_SIZE;
                    
                    // 根据方向计算偏移
                    // 0.0 (Center) -> 0.5 (Edge)
                    // 这里简化：假设都是向东移动
                    // 实际需要根据 input_dir 和 output_dir 做贝塞尔曲线或分段线性插值
                    let offset_x = (path.progress - 0.5) * TILE_SIZE; 
                    
                    transform.translation = (base_pos + Vec2::new(offset_x, 0.0)).extend(1.0);
                }
            }
        }
    }
}