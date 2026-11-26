"""Small generator regressions; no Okkama execution."""
import re
import unittest
import gen


class ChunkTests(unittest.TestCase):
    def setUp(self):
        gen.SEEN.clear()

    def parts(self, program):
        return [body.count('t = t.') for body in re.findall(r'part\d+\(start is Tap\) is Tap \{(.*?)\n    \}', program.render(), re.S)]

    def test_no_setup_is_bounded_by_chunk(self):
        program = gen.Program('no setup')
        for n in range(2 * gen.CHUNK + 1):
            program.is_(str(n), str(n), str(n))
        self.assertEqual(self.parts(program), [40, 40, 1])

    def test_shared_setup_is_not_split(self):
        program = gen.Program('setup')
        for n in range(gen.CHUNK + 1):
            program.is_(f'x + {n}', str(n), str(n), setup=['let x = 0;'] if n == 0 else [])
        program.is_('y', '1', 'new group', setup=['let y = 1;'])
        self.assertEqual(self.parts(program), [41, 1])
        self.assertEqual(program.render().count('let x = 0;'), 1)

    def test_dropped_setup_reaches_first_retained_check(self):
        first = gen.Program('first')
        first.is_('x', '0', 'duplicate', setup=['let x = 0;'])
        second = gen.Program('second')
        second.is_('x', '0', 'duplicate', setup=['let x = 0;'])
        second.is_('x + 1', '1', 'retained')
        self.assertIn('let x = 0;', second.render())
        self.assertEqual(self.parts(second), [1])


if __name__ == '__main__':
    unittest.main()
