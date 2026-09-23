// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Generate, generate_vec, make_rng};
use fidl_next_test_benchmark as ftb_next;
use fidl_test_benchmark as ftb;

use rand::RngExt as _;

impl_generate! {
    for ftb::GameType, ftb_next::GameType => rng {
        match rng.random_range(0..4) {
            0 => Self::Survival,
            1 => Self::Creative,
            2 => Self::Adventure,
            3 => Self::Spectator,
            _ => unreachable!(),
        }
    }
}

const ITEM_IDS: [&str; 8] =
    ["dirt", "stone", "pickaxe", "sand", "gravel", "shovel", "chestplate", "steak"];

impl_generate! {
    for ftb::Item, ftb_next::Item => rng {
        Self {
            count: rng.random(),
            slot: rng.random(),
            id: ITEM_IDS[rng.random_range(0..ITEM_IDS.len())].to_string(),
        }
    }
}

impl_generate! {
    for ftb::Abilities, ftb_next::Abilities => rng {
        Self {
            walk_speed: rng.random(),
            fly_speed: rng.random(),
            may_fly: rng.random_bool(0.5),
            flying: rng.random_bool(0.5),
            invulnerable: rng.random_bool(0.5),
            may_build: rng.random_bool(0.5),
            instabuild: rng.random_bool(0.5),
        }
    }
}

impl_generate! {
    for ftb::Vector3d, ftb_next::Vector3d => rng {
        Self { x: rng.random(), y: rng.random(), z: rng.random() }
    }
}

impl_generate! {
    for ftb::Vector2, ftb_next::Vector2 => rng {
        Self { x: rng.random(), y: rng.random() }
    }
}

const ENTITY_IDS: [&str; 8] =
    ["cow", "sheep", "zombie", "skeleton", "spider", "creeper", "parrot", "bee"];
const CUSTOM_NAMES: [&str; 8] =
    ["rainbow", "princess", "steve", "johnny", "missy", "coward", "fairy", "howard"];

impl_generate! {
    for ftb::Entity, ftb_next::Entity => rng {
        Self {
            id: ENTITY_IDS[rng.random_range(0..ENTITY_IDS.len())].to_string(),
            pos: Generate::generate(rng),
            motion: Generate::generate(rng),
            rotation: Generate::generate(rng),
            fall_distance: rng.random(),
            fire: rng.random(),
            air: rng.random(),
            on_ground: rng.random_bool(0.5),
            no_gravity: rng.random_bool(0.5),
            invulnerable: rng.random_bool(0.5),
            portal_cooldown: rng.random(),
            uuid: Generate::generate(rng),
            custom_name: rng.random_bool(0.5).then(|| CUSTOM_NAMES[rng.random_range(0..CUSTOM_NAMES.len())].to_string()),
            custom_name_visible: rng.random_bool(0.5),
            silent: rng.random_bool(0.5),
            glowing: rng.random_bool(0.5),
        }
    }
}

const RECIPES: [&str; 8] =
    ["pickaxe", "torch", "bow", "crafting table", "furnace", "shears", "arrow", "tnt"];
const MAX_RECIPES: usize = 30;
const MAX_DISPLAYED_RECIPES: usize = 10;

impl_generate! {
    for ftb::RecipeBook, ftb_next::RecipeBook => rng {
        let mut recipes = Vec::new();
        for _ in 0..rng.random_range(0..MAX_RECIPES) {
            recipes.push(RECIPES[rng.random_range(0..RECIPES.len())].to_string());
        }

        let mut to_be_displayed = Vec::new();
        for _ in 0..rng.random_range(0..MAX_DISPLAYED_RECIPES) {
            to_be_displayed.push(RECIPES[rng.random_range(0..RECIPES.len())].to_string());
        }

        Self {
            recipes,
            to_be_displayed,
            is_filtering_craftable: rng.random_bool(0.5),
            is_gui_open: rng.random_bool(0.5),
            is_furnace_filtering_craftable: rng.random_bool(0.5),
            is_furnace_gui_open: rng.random_bool(0.5),
            is_blasting_furnace_filtering_craftable: rng.random_bool(0.5),
            is_blasting_furnace_gui_open: rng.random_bool(0.5),
            is_smoker_filtering_craftable: rng.random_bool(0.5),
            is_smoker_gui_open: rng.random_bool(0.5),
        }
    }
}

impl_generate! {
    for ftb::UuidAndEntity, ftb_next::UuidAndEntity => rng {
        Self {
            uuid: Generate::generate(rng),
            entity: Generate::generate(rng),
        }
    }
}

const DIMENSIONS: [&str; 3] = ["overworld", "nether", "end"];
const MAX_ITEMS: usize = 40;
const MAX_ENDER_ITEMS: usize = 27;

impl_generate! {
    for ftb::Player, ftb_next::Player => rng {
        let inventory_items = rng.random_range(0..MAX_ITEMS);
        let ender_items = rng.random_range(0..MAX_ENDER_ITEMS);

        Self {
            game_type: Generate::generate(rng),
            previous_game_type: Generate::generate(rng),
            score: rng.random(),
            dimension: DIMENSIONS[rng.random_range(0..DIMENSIONS.len())].to_string(),
            selected_item_slot: rng.random(),
            selected_item: Generate::generate(rng),
            spawn_dimension: rng.random_bool(0.5).then(|| DIMENSIONS[rng.random_range(0..DIMENSIONS.len())].to_string()),
            spawn_x: rng.random(),
            spawn_y: rng.random(),
            spawn_z: rng.random(),
            spawn_forced: Generate::generate(rng),
            sleep_timer: rng.random(),
            food_exhaustion_level: rng.random(),
            food_saturation_level: rng.random(),
            food_tick_timer: rng.random(),
            xp_level: rng.random(),
            xp_p: rng.random(),
            xp_total: rng.random(),
            xp_seed: rng.random(),
            inventory: generate_vec(rng, inventory_items),
            ender_items: generate_vec(rng, ender_items),
            abilities: Generate::generate(rng),
            entered_nether_position: Generate::generate(rng),
            root_vehicle: Generate::generate(rng),
            shoulder_entity_left: Generate::generate(rng),
            shoulder_entity_right: Generate::generate(rng),
            seen_credits: rng.random_bool(0.5),
            recipe_book: Generate::generate(rng),
        }
    }
}

pub fn generate_input_rust(input_size: usize) -> ftb::Players {
    let mut rng = make_rng();
    ftb::Players { players: generate_vec(&mut rng, input_size) }
}

pub fn generate_input_rust_next(input_size: usize) -> ftb_next::Players {
    let mut rng = make_rng();
    ftb_next::Players { players: generate_vec(&mut rng, input_size) }
}
