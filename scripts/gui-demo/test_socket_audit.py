import unittest
from pathlib import Path

from socket_audit import check, records


class SocketAuditTests(unittest.TestCase):
    root = Path('/private/tmp/repomon-gui-demo.test')

    def rows(self):
        return records('COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\n'
                       f'Python 10 demo 7u unix 0xa 0t0 {self.root}/app.sock\n'
                       'repomon 20 demo 12u unix 0xb 0t0 ->0xa\n'
                       'repomon 20 demo 9u unix 0xc 0t0 ->0xd\n'
                       'repomon 20 demo 10u unix 0xd 0t0 ->0xc\n')

    def test_accepts_proxy_and_internal_socketpair(self):
        self.assertEqual(len(check(self.rows(), self.root, 20, 10)), 3)

    def test_rejects_production_endpoint_even_if_proxy_owned(self):
        rows = self.rows()
        rows[0]['n'] = '/tmp/repomon-azaleas.sock'
        with self.assertRaises(AssertionError):
            check(rows, self.root, 20, 10)

    def test_rejects_foreign_owner_even_with_sandbox_path(self):
        rows = self.rows()
        rows[0]['pid'] = 99
        with self.assertRaises(AssertionError):
            check(rows, self.root, 20, 10)

    def test_rejects_unknown_peer_and_empty_output(self):
        for rows in (self.rows()[1:], []):
            with self.assertRaises(AssertionError):
                check(rows, self.root, 20, 10)


if __name__ == '__main__':
    unittest.main()
