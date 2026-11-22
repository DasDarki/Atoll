//! Keys that put things in order across devices.
//!
//! A key is a fraction between 0 and 1 written in base 62, without the leading "0." and never
//! ending in the digit zero. Between any two keys there is always another one, so moving a tab
//! gives only that tab a new key and leaves every other key as it was. Two devices that sort at
//! the same time therefore never overwrite each other's order, and every device sorts the keys
//! the same way, byte for byte.

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// A key that sorts after `lower` and before `upper`. `None` stands for the start and the end.
/// Panics when `lower` does not sort before `upper` or a key holds a character that is no digit.
pub fn between(lower: Option<&str>, upper: Option<&str>) -> String {
    let lower = lower.unwrap_or("");
    if let Some(upper) = upper {
        assert!(lower < upper, "{lower:?} does not sort before {upper:?}");
    }
    let digits = |key: &str| -> Vec<u8> { key.bytes().map(value_of).collect() };
    let upper_digits = upper.map(digits);
    let key = midpoint(&digits(lower), upper_digits.as_deref());
    key.into_iter()
        .map(|digit| DIGITS[digit as usize] as char)
        .collect()
}

/// `count` keys in ascending order, spread evenly over the whole range so that later moves find
/// room between them and the keys stay as short as the count allows.
pub fn spread(count: usize) -> Vec<String> {
    let slots = count as u128 + 1;
    let mut width = 1u32;
    while 62u128.pow(width) < slots {
        width += 1;
    }
    let range = 62u128.pow(width);
    (1..=count as u128)
        .map(|index| {
            let mut value = index * range / slots;
            let mut digits = vec![0u8; width as usize];
            for digit in digits.iter_mut().rev() {
                *digit = (value % 62) as u8;
                value /= 62;
            }
            while digits.last() == Some(&0) {
                digits.pop();
            }
            digits
                .into_iter()
                .map(|digit| DIGITS[digit as usize] as char)
                .collect()
        })
        .collect()
}

fn value_of(character: u8) -> u8 {
    DIGITS
        .iter()
        .position(|digit| *digit == character)
        .unwrap_or_else(|| panic!("{:?} is no digit of an order key", character as char)) as u8
}

/// The digits of a fraction between `lower` and `upper`, both given as digits. An empty `lower`
/// is zero, a missing `upper` is one.
fn midpoint(lower: &[u8], upper: Option<&[u8]>) -> Vec<u8> {
    if let Some(upper) = upper {
        let common = upper
            .iter()
            .enumerate()
            .take_while(|(index, digit)| lower.get(*index).copied().unwrap_or(0) == **digit)
            .count();
        if common > 0 {
            let mut key = upper[..common].to_vec();
            let rest_lower = lower.get(common..).unwrap_or(&[]);
            key.extend(midpoint(rest_lower, Some(&upper[common..])));
            return key;
        }
    }
    let low = lower.first().copied().unwrap_or(0);
    let high = upper
        .and_then(|upper| upper.first().copied())
        .map_or(62, u16::from);
    if high - u16::from(low) > 1 {
        return vec![((u16::from(low) + high) / 2) as u8];
    }
    match upper {
        Some(upper) if upper.len() > 1 => vec![upper[0]],
        _ => {
            let mut key = vec![low];
            key.extend(midpoint(lower.get(1..).unwrap_or(&[]), None));
            key
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_lies_between_its_neighbours() {
        assert_eq!(between(None, None), "V");
        let first = between(None, None);
        let after = between(Some(&first), None);
        let before = between(None, Some(&first));
        assert!(before < first && first < after);
        let middle = between(Some(&before), Some(&first));
        assert!(before < middle && middle < first);
    }

    #[test]
    fn there_is_always_room_even_between_close_keys() {
        let mut lower = "V".to_owned();
        let upper = "W".to_owned();
        for _ in 0..200 {
            let key = between(Some(&lower), Some(&upper));
            assert!(lower < key && key < upper, "{lower} {key} {upper}");
            assert!(!key.ends_with('0'));
            lower = key;
        }
    }

    #[test]
    fn inserting_anywhere_keeps_every_key_in_order() {
        let mut keys = spread(5);
        let mut seed = 7u64;
        for _ in 0..500 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let at = (seed >> 33) as usize % (keys.len() + 1);
            let lower = at.checked_sub(1).map(|index| keys[index].as_str());
            let upper = keys.get(at).map(String::as_str);
            let key = between(lower, upper);
            keys.insert(at, key);
        }
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            keys.iter()
                .all(|key| !key.ends_with('0') && !key.is_empty())
        );
    }

    #[test]
    fn spread_keys_ascend_and_stay_short() {
        for count in [0, 1, 2, 61, 62, 100, 5000] {
            let keys = spread(count);
            assert_eq!(keys.len(), count);
            assert!(keys.windows(2).all(|pair| pair[0] < pair[1]), "{count}");
            assert!(
                keys.iter()
                    .all(|key| !key.is_empty() && !key.ends_with('0'))
            );
        }
        assert!(spread(100).iter().all(|key| key.len() <= 2));
        assert_eq!(spread(1), ["V"]);
    }

    #[test]
    #[should_panic]
    fn neighbours_in_the_wrong_order_are_refused() {
        between(Some("b"), Some("a"));
    }
}
