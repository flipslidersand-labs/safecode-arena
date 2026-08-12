def add(a: int, b: int) -> int:
    return a + b


def sum_values(values: list[int]) -> int:
    return sum(values)


def fibonacci(n: int) -> int:
    if n < 0:
        raise ValueError("n must be non-negative")
    if n == 0:
        return 0
    a, b = 0, 1
    for _ in range(1, n):
        a, b = b, a + b
    return b


def test_add_positive():
    assert add(2, 3) == 5


def test_add_negative():
    assert add(-1, 1) == 0


def test_sum_empty():
    assert sum_values([]) == 0


def test_sum_values():
    assert sum_values([1, 2, 3, 4]) == 10


def test_fibonacci_base():
    assert fibonacci(0) == 0
    assert fibonacci(1) == 1


def test_fibonacci_tenth():
    assert fibonacci(10) == 55
