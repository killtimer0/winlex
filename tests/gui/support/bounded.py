"""Run blocking Python work under an absolute diagnostic deadline."""
import threading
import traceback


class Task:
    def __init__(self):
        self.value = None
        self.error = None
        self.traceback = None
        self.finished = threading.Event()

    def wait(self, seconds, description):
        if not self.finished.wait(seconds):
            raise TimeoutError('%s exceeded %s seconds' % (description, seconds))
        if self.error is not None:
            raise self.error
        return self.value


def start(function):
    task = Task()

    def run():
        try:
            task.value = function()
        except BaseException as error:
            task.error = error
            task.traceback = traceback.format_exc()
        finally:
            task.finished.set()

    threading.Thread(target=run, daemon=True).start()
    return task


def call(function, seconds, description):
    return start(function).wait(seconds, description)
