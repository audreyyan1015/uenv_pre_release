import random

def test_case_input_generator(n=200):
    test_cases = []
    for _ in range(n):
        a = random.randint(-1000, 1000)
        b = random.randint(-1000, 1000)
        test_cases.append((a, b))
    return test_cases
