use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};

use bevy::color::Srgba;
use serde::Deserialize;
use std::fs;

// 必须 derive Deserialize 才能从 TOML 字符串 "North" 自动转换
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    North,
    East,
    South,
    West,
}

impl Default for Direction {
    fn default() -> Self {
        Direction::North
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Shape {
    width: u32,
    height: u32,
}

// 机器端口定义 (预计算用)
#[derive(Debug, Clone)]
struct ComponentPort {
    offset: IVec2,      // 相对坐标 (未旋转)
    direction: Direction, // 朝向
    r#type: PortType,   // Input 或 Output
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PortType {
    None,
    Input,
    Output,
}

// 机器专门的核心逻辑组件（不包含位置！）
#[derive(Component)]
pub struct MachineCore {
    pub prototype_id: String,
    pub is_working: bool,
}

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

// 把网格相关的属性打包成一个组件
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GridTransform {
    pub position: IVec2, // 绝对坐标 [x, y]
    pub direction: Direction, // 朝向
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

// 地图索引：用于快速查找某个格子上有什么
#[derive(Resource, Default)]
struct GridMap {
    // 存储 Entity ID
    entities: HashMap<IVec2, Entity>,
}

#[derive(Debug, Clone)]
struct Recipe {
    inputs: Vec<RecipeItem>,
    outputs: Vec<RecipeItem>,
    time: f32,
}

#[derive(Debug, Clone)]
struct ItemPrototype {
    id: String,
    color: Color,
    description: String,
}

#[derive(Debug, Clone)]
struct MachinePrototype {
    id: String,
    shape: Shape,
    color: Color,
    recipes: Vec<Recipe>,
    speed_modifier: f32,
    // 🔥 变更: 不再存 layout 矩阵，改为存端口列表
    ports: Vec<ComponentPort>,
}

#[derive(Debug, Clone)]
struct BeltPrototype {
    id: String,
    color: Color,
    speed: f32,
    inputs: ComponentPort,
    outputs: ComponentPort,
}

#[derive(Resource, Default)]
struct PrototypeLibrary {
    machines: HashMap<String, MachinePrototype>,
    items: HashMap<String, ItemPrototype>,
    belts: HashMap<String, BeltPrototype>, // ✅ 新增
}



fn spawn_machine(
    commands: &mut Commands,
    map: &mut ResMut<GridMap>,
    proto_lib: &Res<PrototypeLibrary>,
    asset_server: &Res<AssetServer>,
    pos: IVec2,
    proto_id: String,
    dir: Direction,
) {
    // 1. 获取原型数据
    let proto = proto_lib.machines.get(&proto_id)
        .expect(&format!("未找到机器原型: {}", proto_id));

    // 2. 计算机器占据的总物理尺寸 (如果是长方形，旋转 90 度后宽高会互换)
    // 假设原型里的 width/height 是 North 朝向时的原始尺寸
    let current_size = if dir == Direction::East || dir == Direction::West {
        IVec2::new(proto.height as i32, proto.width as i32)
    } else {
        IVec2::new(proto.width as i32, proto.height as i32)
    };

    // 3. 计算视觉旋转 (Sprite 需要弧度)
    let rotation_angle = match dir {
        Direction::North => 0.0,
        Direction::East => -std::f32::consts::FRAC_PI_2,
        Direction::South => std::f32::consts::PI,
        Direction::West => std::f32::consts::FRAC_PI_2,
    };

    // 4. 生成实体
    let machine_ent = commands
        .spawn((
            // --- 空间状态组件 ---
            GridTransform {
                position: pos,
                direction: dir,
            },
            // --- 核心逻辑组件 ---
            MachineCore {
                prototype_id: proto_id.clone(),
                is_working: false,
            },
            // --- 视觉组件 ---
            Sprite {
                color: proto.color,
                // 注意：Sprite 尺寸永远使用原型的原始尺寸，旋转由 Transform 处理
                custom_size: Some(Vec2::new(proto.width as f32, proto.height as f32) * TILE_SIZE - 2.0),
                ..default()
            },
            Transform {
                // 计算中心点：将 (0,0) 的左下角对齐转换为中心点对齐
                translation: Vec3::new(
                    (pos.x as f32 + current_size.x as f32 / 2.0 - 0.5) * TILE_SIZE,
                    (pos.y as f32 + current_size.y as f32 / 2.0 - 0.5) * TILE_SIZE,
                    0.1, // 略高于传送带
                ),
                rotation: Quat::from_rotation_z(rotation_angle),
                ..default()
            },
            Visibility::Visible,
            InheritedVisibility::default(),
            GlobalTransform::default(),
        ))
        .with_children(|parent| {
            // 可以在这里生成端口指示器或调试文字
            spawn_machine_debug_info(parent, asset_server, &proto_id);
        })
        .id();

    // 5. 更新全局网格索引 (占用多个格子)
    for x in 0..current_size.x {
        for y in 0..current_size.y {
            let tile_pos = pos + IVec2::new(x, y);
            map.entities.insert(tile_pos, machine_ent);
        }
    }
    
    info!("✅ 已生成机器: {} 于 {:?}", proto_id, pos);
}

// 辅助：生成机器上的标签
fn spawn_machine_debug_info(parent: &mut ChildBuilder, asset_server: &Res<AssetServer>, id: &str) {
    parent.spawn((
        Text2d::new(id),
        TextFont {
            font: asset_server.load("fonts/FiraSans-Bold.ttf"),
            font_size: 12.0,
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, 1.0), // 确保文字在机器上方
    ));
}
fn main() {
    print!("Hello, world!");
}