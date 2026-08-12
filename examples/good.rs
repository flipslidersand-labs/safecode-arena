/// 2 つの整数を加算する。
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

/// 整数のスライスの合計を返す。空スライスは 0。
pub fn sum(values: &[i32]) -> i32 {
    values.iter().sum()
}

/// n 番目のフィボナッチ数を返す（0-indexed）。
pub fn fibonacci(n: u32) -> u64 {
    match n {
        0 => 0,
        1 => 1,
        _ => {
            let (mut a, mut b) = (0u64, 1u64);
            for _ in 2..=n {
                (a, b) = (b, a + b);
            }
            b
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_positive() {
        assert_eq!(add(2, 3), 5);
    }

    #[test]
    fn add_negative() {
        assert_eq!(add(-1, 1), 0);
    }

    #[test]
    fn sum_empty() {
        assert_eq!(sum(&[]), 0);
    }

    #[test]
    fn sum_values() {
        assert_eq!(sum(&[1, 2, 3, 4]), 10);
    }

    #[test]
    fn fibonacci_base_cases() {
        assert_eq!(fibonacci(0), 0);
        assert_eq!(fibonacci(1), 1);
    }

    #[test]
    fn fibonacci_tenth() {
        assert_eq!(fibonacci(10), 55);
    }
}
