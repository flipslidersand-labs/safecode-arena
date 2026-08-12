package main

import "fmt"

func add(a, b int) int {
	return a + b
}

func sumValues(values []int) int {
	total := 0
	for _, v := range values {
		total += v
	}
	return total
}

func fibonacci(n int) (int, error) {
	if n < 0 {
		return 0, fmt.Errorf("n must be non-negative")
	}
	if n == 0 {
		return 0, nil
	}
	a, b := 0, 1
	for i := 1; i < n; i++ {
		a, b = b, a+b
	}
	return b, nil
}
