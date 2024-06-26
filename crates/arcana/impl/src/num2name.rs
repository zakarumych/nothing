use std::hash::Hash;

use crate::stable_hash;

const ADJECTIVES: &'static [&'static str; 32] = &[
    "Brave", "Bright", "Calm", "Clever", "Cool", "Cozy", "Cute", "Eager", "Fancy", "Fresh", "Good",
    "Grace", "Great", "Happy", "Hardy", "Jolly", "Joyful", "Keen", "Kind", "Lively", "Lucky",
    "Merry", "Nice", "Shiny", "Smart", "Spark", "Stellar", "Sweet", "Swift", "Warm", "Wise", "Zen",
];

const COLORS: &'static [&'static str; 32] = &[
    "Aqua", "Beige", "Black", "Blue", "Brass", "Bronze", "Coral", "Cyan", "Gold", "Gray", "Green",
    "Indigo", "Ivory", "Ivory", "Jet", "Lemon", "Lilac", "Lime", "Maroon", "Mint", "Onyx", "Opal",
    "Pearl", "Pink", "Plum", "Red", "Rose", "Ruby", "Silver", "Tan", "Teal", "Topaz",
];

const NOUNS: &'static [&'static str; 64] = &[
    "Axolotl", "Bat", "Bear", "Bee", "Boar", "Cat", "Chimp", "Cobra", "Crab", "Crow", "Deer",
    "Dog", "Dolphin", "Dove", "Finch", "Fish", "Fox", "Frog", "Gull", "Hawk", "Horse", "Jay",
    "Koala", "Lion", "Lynx", "Marmoset", "Mink", "Moth", "Newt", "Owl", "Pangolin", "Pika", "Puma",
    "Quail", "Quokka", "Raccoon", "Rat", "Seal", "Shrew", "Skunk", "Snail", "Sparrow", "Stoat",
    "Swan", "Tern", "Toad", "Viper", "Vole", "Wombat", "Wolf", "Yak", "Zebra", "Zorse", "Axolotl",
    "Bear", "Cat", "Dog", "Fox", "Hawk", "Koala", "Lion", "Mink", "Owl", "Seal",
];

pub fn num_to_name(num: u16) -> String {
    let num = (((num as u32) * 29983u32) >> 8) as u16;

    let adjective = (num >> 11) as usize;
    let color = ((num >> 6) & 0b11111) as usize;
    let noun = (num & 0b111111) as usize;

    format!(
        "{} {} {}",
        ADJECTIVES[adjective], COLORS[color], NOUNS[noun]
    )
}

pub fn hash_to_name<T>(value: &T) -> String
where
    T: Hash + ?Sized,
{
    let hash = stable_hash(value);
    let num = (hash >> 48) as u16;
    num_to_name(num)
}
