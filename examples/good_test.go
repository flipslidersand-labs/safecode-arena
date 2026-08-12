package main

import "testing"

func TestAddPositive(t *testing.T) {
	if got := add(2, 3); got != 5 {
		t.Errorf("add(2,3) = %d, want 5", got)
	}
}

func TestAddNegative(t *testing.T) {
	if got := add(-1, 1); got != 0 {
		t.Errorf("add(-1,1) = %d, want 0", got)
	}
}

func TestSumEmpty(t *testing.T) {
	if got := sumValues([]int{}); got != 0 {
		t.Errorf("sumValues([]) = %d, want 0", got)
	}
}

func TestSumValues(t *testing.T) {
	if got := sumValues([]int{1, 2, 3, 4}); got != 10 {
		t.Errorf("sumValues([1,2,3,4]) = %d, want 10", got)
	}
}

func TestFibonacciBase(t *testing.T) {
	for _, tc := range []struct{ n, want int }{{0, 0}, {1, 1}} {
		got, err := fibonacci(tc.n)
		if err != nil || got != tc.want {
			t.Errorf("fibonacci(%d) = %d, %v; want %d, nil", tc.n, got, err, tc.want)
		}
	}
}

func TestFibonacciTenth(t *testing.T) {
	got, err := fibonacci(10)
	if err != nil || got != 55 {
		t.Errorf("fibonacci(10) = %d, %v; want 55, nil", got, err)
	}
}
