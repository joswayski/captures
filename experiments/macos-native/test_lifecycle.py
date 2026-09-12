import pathlib
import unittest


ROOT = pathlib.Path(__file__).parent


class LifecycleSourceTests(unittest.TestCase):
    def test_restart_helper_is_prepared_before_clean_exit_and_failure_resumes_tracking(self):
        source = (ROOT / "Sources/CapturesNative/Main.swift").read_text()
        finish = source[source.index("private func finishTermination"):source.index("private func completeTermination")]

        self.assertLess(finish.index("try prepareRestartWaiter()"), finish.index('Backend.shared.call("crash_mark_clean")'))
        failure = finish[finish.index("case .failure(let error):"):]
        self.assertLess(failure.index('Backend.shared.call("crash_resume")'), failure.index("self.cancelTermination(sender)"))


if __name__ == "__main__":
    unittest.main()
