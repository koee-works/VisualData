#!/usr/bin/env python3
"""End-to-end test of the real desktop app (Tauri + WebKitGTK + vda-core).

Drives the app through tauri-driver / WebKitWebDriver with the W3C WebDriver
protocol (standard library only) and walks the shared acceptance scenario:
open a project, run, inspect results and SQL, edit a step and see downstream
results go outdated, rerun, read the notebook, view the chart.

Usage: run_e2e.py <app-binary> <project-dir> <screenshot-dir>
Needs a display (Xvfb in CI) and tauri-driver on PATH.
"""

import base64
import http.client
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

DRIVER = "http://127.0.0.1:4444"


def call(method, path, body=None):
    data = json.dumps(body).encode() if body is not None else None
    for attempt in range(6):
        req = urllib.request.Request(DRIVER + path, data=data, method=method, headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=120) as r:
                return json.loads(r.read() or b"{}").get("value")
        except urllib.error.HTTPError as e:
            raise RuntimeError(f"{method} {path}: {e.code} {e.read().decode()[:400]}") from None
        except (ConnectionError, http.client.RemoteDisconnected, urllib.error.URLError):
            # tauri-driver occasionally drops a connection between requests.
            if attempt == 5:
                raise
            time.sleep(0.5 * (attempt + 1))


class Session:
    def __init__(self, app):
        caps = {"capabilities": {"alwaysMatch": {"tauri:options": {"application": app}}}}
        self.id = call("POST", "/session", caps)["sessionId"]

    def find(self, xpath, timeout=20):
        end = time.time() + timeout
        last = None
        while time.time() < end:
            try:
                el = call("POST", f"/session/{self.id}/element", {"using": "xpath", "value": xpath})
                return list(el.values())[0]
            except RuntimeError as e:
                last = e
                time.sleep(0.25)
        raise AssertionError(f"not found within {timeout}s: {xpath}\n{last}")

    def click(self, xpath):
        call("POST", f"/session/{self.id}/element/{self.find(xpath)}/click", {})

    def text(self, xpath):
        # textContent, not WebDriver's visible text: WebKit's driver drops
        # text in cells that truncate with an ellipsis.
        el = self.find(xpath)
        ref = {"element-6066-11e4-a52e-4f735466cecf": el}
        return call("POST", f"/session/{self.id}/execute/sync", {"script": "return arguments[0].textContent", "args": [ref]})

    def wait_text(self, xpath, contains, timeout=60):
        end = time.time() + timeout
        while time.time() < end:
            try:
                if contains in self.text(xpath):
                    return
            except (RuntimeError, AssertionError):
                pass
            time.sleep(0.3)
        raise AssertionError(f"{xpath!r} never contained {contains!r}; last: {self.text(xpath)!r}")

    def clear_and_type(self, xpath, value):
        el = self.find(xpath)
        call("POST", f"/session/{self.id}/element/{el}/clear", {})
        call("POST", f"/session/{self.id}/element/{el}/value", {"text": value})

    def count(self, xpath):
        return len(call("POST", f"/session/{self.id}/elements", {"using": "xpath", "value": xpath}))

    def screenshot(self, path):
        time.sleep(0.6)  # let transitions and charts settle
        png = call("GET", f"/session/{self.id}/screenshot")
        with open(path, "wb") as f:
            f.write(base64.b64decode(png))
        print(f"  screenshot {os.path.basename(path)}")

    def close(self):
        try:
            call("DELETE", f"/session/{self.id}")
        except RuntimeError:
            pass


def node(title):
    """A pipeline node by its visible title."""
    return f"//nav[@aria-label='Pipeline']//button[.//span[contains(normalize-space(.), {json.dumps(title)})]]"


STATUS = "//header//p[@role='status']"


def main():
    app, project, shots = sys.argv[1:4]
    os.makedirs(shots, exist_ok=True)
    env = dict(os.environ, VDA_OPEN=project)
    driver = subprocess.Popen(["tauri-driver", "--port", "4444"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.STDOUT)
    s = None
    try:
        for _ in range(60):
            try:
                call("GET", "/status")
                break
            except Exception:
                time.sleep(0.25)
        s = Session(app)
        step = lambda n, msg: print(f"{n}. {msg}")

        step(1, "Open project: steps exist but have not run")
        s.wait_text(STATUS, "need a run")
        n = s.count("//nav[@aria-label='Pipeline']//span[@role='img' and @aria-label='Not run']")
        assert n == 4, f"four steps start not run (found {n})"
        s.find("//section[@aria-label='Source']//td[normalize-space()='order_date']")
        s.screenshot(f"{shots}/1-opened.png")

        step(2, "Run the analysis; every step becomes current")
        s.click("//button[normalize-space()='Run analysis']")
        s.wait_text(STATUS, "All results current")
        assert s.count("//nav[@aria-label='Pipeline']//span[@role='img' and @aria-label='Current']") == 4

        step(3, "Inspect a visual step: its result and the SQL it runs")
        s.click(node("Revenue by region"))
        s.wait_text("//div[@role='table']", "East")
        assert "19" in s.text("//div[@role='table']"), "East has 19 orders"
        s.screenshot(f"{shots}/2-aggregate-result.png")
        s.click("//button[@role='tab' and normalize-space()='SQL']")
        s.wait_text("//pre", 'GROUP BY "region"')
        s.screenshot(f"{shots}/3-aggregate-sql.png")

        step(4, "Edit the filter: it and everything after it become outdated")
        s.click(node("Orders with a region"))
        s.clear_and_type("//input[@aria-label='Value']", "100")
        s.click("//button[normalize-space()='Save step']")
        s.wait_text(STATUS, "4 of 4 results need a run")
        assert s.count("//nav[@aria-label='Pipeline']//span[@role='img' and @aria-label='Outdated']") == 4
        s.click(node("Revenue by region"))
        s.find("//*[@role='status' and contains(., 'Outdated result')]")
        s.screenshot(f"{shots}/4-outdated.png")

        step(5, "Rerun from the outdated banner; results are current again")
        s.click("//button[normalize-space()='Run now']")
        s.wait_text(STATUS, "All results current")
        assert s.count("//*[contains(., 'Outdated result')]") == 0

        step(6, "Notebook: text and step results in document order")
        s.click("//button[@role='tab' and normalize-space()='Notebook']")
        s.find("//article[@aria-label='Notebook']//h3[normalize-space()='Question']")
        s.find("//article[@aria-label='Notebook']//figure//*[contains(@class,'vega-embed')]//*[local-name()='svg']")
        s.screenshot(f"{shots}/5-notebook.png")

        step(7, "Chart step")
        s.click("//button[@role='tab' and normalize-space()='Step']")
        s.click(node("Revenue share by region"))
        s.find("//section[@aria-label='Result']//*[contains(@class,'vega-embed')]//*[local-name()='svg']")
        s.screenshot(f"{shots}/6-chart.png")

        step(8, "Python step shows its code and contract")
        s.click(node("Revenue share"))
        s.find("//textarea[@aria-label='Python code']")
        s.screenshot(f"{shots}/7-python.png")
        print("PASS: end-to-end scenario")
    except BaseException:
        if s:
            try:
                s.screenshot(f"{shots}/failure.png")
            except Exception:
                pass
        raise
    finally:
        if s:
            s.close()
        driver.terminate()
        driver.wait(timeout=10)


if __name__ == "__main__":
    main()
