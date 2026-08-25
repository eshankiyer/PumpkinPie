use std::num::NonZero;

use pumpkin_util::{
    random::{RandomGenerator, RandomImpl},
    y_offset::YOffset,
};
use tracing::warn;

pub enum HeightProvider {
    Uniform(UniformHeightProvider),
    Trapezoid(TrapezoidHeightProvider),
    BiasedToBottom(BiasedToBottomHeightProvider),
    VeryBiasedToBottom(VeryBiasedToBottomHeightProvider),
    WeightedList(WeightedListHeightProvider),
}

impl HeightProvider {
    pub fn get(&self, random: &mut RandomGenerator, min_y: i8, height: u16) -> i32 {
        match self {
            Self::Uniform(provider) => provider.get(random, min_y, height),
            Self::Trapezoid(provider) => provider.get(random, min_y, height),
            Self::BiasedToBottom(provider) => provider.get(random, min_y, height),
            Self::VeryBiasedToBottom(provider) => provider.get(random, min_y, height),
            Self::WeightedList(provider) => provider.get(random, min_y, height),
        }
    }
}

pub struct WeightedListHeightProvider {
    pub distribution: Vec<WeightedHeightEntry>,
}

pub struct WeightedHeightEntry {
    pub data: HeightProvider,
    pub weight: i32,
}

impl WeightedListHeightProvider {
    /// Selects one weighted provider and samples it, matching
    /// `WeightedListHeight.sample` (`WeightedListHeight.java:20-22`).
    pub fn get(&self, random: &mut RandomGenerator, min_y: i8, height: u16) -> i32 {
        let total_weight: i32 = self.distribution.iter().map(|entry| entry.weight).sum();
        assert!(
            total_weight > 0,
            "weighted height provider must not be empty"
        );

        let mut selection = random.next_bounded_i32(total_weight);
        for entry in &self.distribution {
            selection -= entry.weight;
            if selection < 0 {
                return entry.data.get(random, min_y, height);
            }
        }

        unreachable!("weighted height provider selection exceeded its total weight")
    }
}

pub struct BiasedToBottomHeightProvider {
    pub min_inclusive: YOffset,
    pub max_inclusive: YOffset,
    pub inner: Option<NonZero<u32>>,
}

impl BiasedToBottomHeightProvider {
    /// Samples the lower-biased vertical range from vanilla `sample`.
    ///
    /// Source: `net/minecraft/world/level/levelgen/heightproviders/BiasedToBottomHeight.java:37-46`.
    pub fn get(&self, random: &mut RandomGenerator, min_y: i8, height: u16) -> i32 {
        let min = self.min_inclusive.get_y(min_y as i16, height);
        let max = self.max_inclusive.get_y(min_y as i16, height);
        let inner = self.inner.map_or(1, std::num::NonZero::get) as i32;

        if max - min - inner < 0 {
            warn!("Empty height range");
            return min;
        }

        let limit = random.next_bounded_i32(max - min - inner + 1);
        random.next_bounded_i32(limit + inner) + min
    }
}

pub struct VeryBiasedToBottomHeightProvider {
    pub min_inclusive: YOffset,
    pub max_inclusive: YOffset,
    pub inner: Option<NonZero<u32>>,
}

impl VeryBiasedToBottomHeightProvider {
    pub fn get(&self, random: &mut RandomGenerator, min_y: i8, height: u16) -> i32 {
        let min = self.min_inclusive.get_y(min_y as i16, height);
        let max = self.max_inclusive.get_y(min_y as i16, height);
        let inner = self.inner.map_or(1, std::num::NonZero::get) as i32;

        let min_rnd = random.next_inbetween_i32(min + inner, max);
        let max_rnd = random.next_inbetween_i32(min, min_rnd - 1);

        random.next_inbetween_i32(min, max_rnd - 1 + inner)
    }
}

pub struct UniformHeightProvider {
    pub min_inclusive: YOffset,
    pub max_inclusive: YOffset,
}

impl UniformHeightProvider {
    pub fn get(&self, random: &mut RandomGenerator, min_y: i8, height: u16) -> i32 {
        let min = self.min_inclusive.get_y(min_y as i16, height);
        let max = self.max_inclusive.get_y(min_y as i16, height);

        random.next_inbetween_i32(min, max)
    }
}

pub struct TrapezoidHeightProvider {
    pub min_inclusive: YOffset,
    pub max_inclusive: YOffset,
    pub plateau: Option<i32>,
}

impl TrapezoidHeightProvider {
    pub fn get(&self, random: &mut RandomGenerator, min_y: i8, height: u16) -> i32 {
        let plateau = self.plateau.unwrap_or(0);
        let i = self.min_inclusive.get_y(min_y as i16, height);
        let j = self.max_inclusive.get_y(min_y as i16, height);

        if i > j {
            warn!("Empty height range");
            return i;
        }

        let k = j - i;
        if plateau >= k {
            return random.next_inbetween_i32(i, j);
        }

        let l = (k - plateau) / 2;
        let m = k - l;
        i + random.next_inbetween_i32(0, m) + random.next_inbetween_i32(0, l)
    }
}
