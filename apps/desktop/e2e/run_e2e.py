#!/usr/bin/env python3
"""End-to-end test of the real desktop app (Tauri + WebKitGTK + vda-core).

Drives the app through tauri-driver / WebKitWebDriver with the W3C WebDriver
protocol (standard library only) and walks the shared acceptance scenario:
open a project received from someone else and review its Python before it
can run (ADR-0004), run, inspect results and SQL, edit a step and see downstream
results go outdated, rerun, read the notebook, view the chart.

Then, with a second project (two sources, a join, and SQL reading two
inputs): run, inspect the join and its SQL, change the join kind and see the
steps after it go outdated, rerun, and look at the second source.

Usage: run_e2e.py <app-binary> <project-dir> <screenshot-dir> [<join-project-dir>]
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
        # Editors scroll inside their panel; WebKit's driver does not scroll there itself.
        ref = {"element-6066-11e4-a52e-4f735466cecf": el}
        call("POST", f"/session/{self.id}/execute/sync", {"script": "arguments[0].scrollIntoView({block: 'center'})", "args": [ref]})
        call("POST", f"/session/{self.id}/element/{el}/clear", {})
        call("POST", f"/session/{self.id}/element/{el}/value", {"text": value})

    def count(self, xpath):
        return len(call("POST", f"/session/{self.id}/elements", {"using": "xpath", "value": xpath}))

    def wait_gone(self, xpath, timeout=10):
        end = time.time() + timeout
        while self.count(xpath):
            if time.time() > end:
                raise AssertionError(f"still present after {timeout}s: {xpath}")
            time.sleep(0.2)

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
    # Titles are the first truncated line of a node; "with …" lines are not titles.
    return (
        "//nav[@aria-label='Pipeline']//button[.//span[contains(@class,'truncate') and "
        f"not(starts-with(normalize-space(.), 'with ')) and starts-with(normalize-space(.), {json.dumps(title)})]]"
    )


STATUS = "//header//p[@role='status']"


def with_app(app, project, shots, approvals, scenario):
    """Start the app on `project` through tauri-driver and run `scenario(s)`."""
    env = dict(os.environ, VDA_OPEN=project, VDA_APPROVALS=approvals)
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
        scenario(s)
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


def step(n, msg):
    print(f"{n}. {msg}")


def main():
    app, project, shots = sys.argv[1:4]
    join_project = sys.argv[4] if len(sys.argv) > 4 else None
    os.makedirs(shots, exist_ok=True)
    # A fresh approvals file: this "user" has approved nothing yet.
    approvals = os.path.join(shots, "approvals.json")
    if os.path.exists(approvals):
        os.remove(approvals)
    with_app(app, project, shots, approvals, lambda s: received_project(s, shots))
    if join_project:
        with_app(app, join_project, shots, approvals, lambda s: joins(s, shots))
    print("PASS: end-to-end scenario")


def received_project(s, shots):
    if True:  # (indented block kept from the single-scenario version)

        mark = lambda label: f"//nav[@aria-label='Pipeline']//span[@role='img' and @aria-label={json.dumps(label)}]"
        dialog = "//*[@role='dialog']"

        step(1, "Open a received project: its Python code is shown for review first")
        s.wait_text(dialog, "before it runs")
        assert "df.assign" in s.text(dialog + "//pre"), "the exact code is shown"
        s.screenshot(f"{shots}/0-review.png")
        s.click(dialog + "//button[normalize-space()='Not now']")
        assert s.count(mark("Not run")) == 2 and s.count(mark("Needs your review")) == 1
        assert s.count(mark("Waiting on a review")) == 1
        s.find("//section[@aria-label='Source']//td[normalize-space()='order_date']")
        s.screenshot(f"{shots}/1-opened.png")

        step(2, "Run without approving: visual steps run, the Python step and the chart after it do not")
        s.click("//button[normalize-space()='Run analysis']")
        s.wait_text(STATUS, "2 of 4 results need a run")
        assert s.count(mark("Current")) == 2
        assert s.count(mark("Needs your review")) == 1 and s.count(mark("Waiting on a review")) == 1

        step(3, "Approve from the top bar and run; every step becomes current")
        s.click("//header//button[contains(normalize-space(), 'Review 1 Python step')]")
        s.click(dialog + "//button[normalize-space()='Approve']")
        s.wait_text(STATUS, "need a run")
        s.wait_gone(dialog)  # the dialog closes when nothing is left to review
        s.click("//button[normalize-space()='Run analysis']")
        s.wait_text(STATUS, "All results current")
        assert s.count(mark("Current")) == 4

        step(4, "Inspect a visual step: its result and the SQL it runs")
        s.click(node("Revenue by region"))
        s.wait_text("//div[@role='table']", "East")
        assert "19" in s.text("//div[@role='table']"), "East has 19 orders"
        s.screenshot(f"{shots}/2-aggregate-result.png")
        s.click("//button[@role='tab' and normalize-space()='SQL']")
        s.wait_text("//pre", 'GROUP BY "region"')
        s.screenshot(f"{shots}/3-aggregate-sql.png")

        step(5, "Edit the filter: it and everything after it become outdated")
        s.click(node("Orders with a region"))
        s.clear_and_type("//input[@aria-label='Value']", "100")
        s.click("//button[normalize-space()='Save step']")
        s.wait_text(STATUS, "4 of 4 results need a run")
        assert s.count("//nav[@aria-label='Pipeline']//span[@role='img' and @aria-label='Outdated']") == 4
        s.click(node("Revenue by region"))
        s.find("//*[@role='status' and contains(., 'Outdated result')]")
        s.screenshot(f"{shots}/4-outdated.png")

        step(6, "Rerun from the outdated banner; results are current again")
        s.click("//button[normalize-space()='Run now']")
        s.wait_text(STATUS, "All results current")
        assert s.count("//*[contains(., 'Outdated result')]") == 0

        step(7, "Notebook: text and step results in document order")
        s.click("//button[@role='tab' and normalize-space()='Notebook']")
        s.find("//article[@aria-label='Notebook']//h3[normalize-space()='Question']")
        s.find("//article[@aria-label='Notebook']//figure//*[contains(@class,'vega-embed')]//*[local-name()='svg']")
        s.screenshot(f"{shots}/5-notebook.png")

        step(8, "Chart step")
        s.click("//button[@role='tab' and normalize-space()='Step']")
        s.click(node("Revenue share by region"))
        s.find("//section[@aria-label='Result']//*[contains(@class,'vega-embed')]//*[local-name()='svg']")
        s.screenshot(f"{shots}/6-chart.png")

        step(9, "Python step shows its code and contract")
        s.click(node("Revenue share"))
        s.find("//textarea[@aria-label='Python code']")
        s.screenshot(f"{shots}/7-python.png")


def joins(s, shots):
    mark = lambda label: f"//nav[@aria-label='Pipeline']//span[@role='img' and @aria-label={json.dumps(label)}]"
    table = "//div[@role='table']"

    step(10, "Open a project with several sources: each is a root, the join says what else it reads")
    s.wait_text(STATUS, "need a run")
    s.find(node("regions.csv"))
    assert "with regions.csv" in s.text(node("Revenue against target"))

    step(11, "Run: the full join keeps a region with no orders; its SQL is a full outer join")
    s.click("//button[normalize-space()='Run analysis']")
    s.wait_text(STATUS, "All results current")
    assert s.count(mark("Current")) == 6
    s.click(node("Revenue against target"))
    s.wait_text(table, "Central")
    assert "Mei Chen" in s.text(table)
    s.screenshot(f"{shots}/8-join.png")
    s.click("//button[@role='tab' and normalize-space()='SQL']")
    s.wait_text("//pre", "FULL OUTER JOIN")

    step(12, "Keep only matching rows: the join and the step after it become outdated, then rerun")
    s.click("//button[@role='tab' and normalize-space()='Data']")
    s.click("//label[.//span[normalize-space()='Only rows that match']]")
    s.click("//button[normalize-space()='Save step']")
    s.wait_text(STATUS, "4 of 6 results need a run")
    s.click("//button[normalize-space()='Run analysis']")
    s.wait_text(STATUS, "All results current")
    s.wait_text(node("Revenue against target"), "4 rows")
    s.wait_text(table, "East")
    assert "Central" not in s.text(table)

    step(13, "Custom SQL reads the second source by name")
    s.click(node("Regions below target"))
    s.find("//section[@aria-label='Step editor']//p//code[normalize-space()='regions']")
    s.wait_text(table, "Tom Okafor")  # North, South and West are below target; East is not
    assert "Priya Raman" not in s.text(table)
    s.screenshot(f"{shots}/9-sql-two-inputs.png")

    step(14, "The second source has its own profile")
    s.click(node("regions.csv"))
    s.find("//section[@aria-label='Source']//td[normalize-space()='manager']")

    step(15, "A spreadsheet is a source: its sheet, typed columns, and an append that stacks it with a CSV")
    s.click(node("regions.xlsx"))
    s.wait_text("//section[@aria-label='Source']", "Spreadsheet file")
    s.find("//section[@aria-label='Source']//td[normalize-space()='since']/following-sibling::td[normalize-space()='date']")
    s.click(node("Regions from both files"))
    s.wait_text(table, "from_file")
    assert "sheet" in s.text(table) and "regions" in s.text(table)
    s.screenshot(f"{shots}/10-append.png")

    step(16, "Rename a source: the steps and the SQL that name it follow, and run again")
    s.click(node("regions.csv"))
    s.clear_and_type("//input[@id='source-name']", "targets")
    s.click("//button[normalize-space()='Rename']")
    s.wait_text(node("regions.csv"), "as targets")
    s.wait_text(STATUS, "need a run")
    s.click("//button[normalize-space()='Run analysis']")
    s.wait_text(STATUS, "All results current")
    s.click(node("Regions below target"))
    s.click("//button[@role='tab' and normalize-space()='SQL']")
    s.wait_text("//pre", "FROM targets")

    step(17, "Calculated columns: a formula error says what is wrong; fixed, it runs and the sort keeps the top two")
    s.click(node("Progress against target"))
    s.click("//button[@role='tab' and normalize-space()='Data']")
    s.wait_text(table, "share_of_target")
    formula = "(//textarea[@aria-label='Formula'])[2]"
    s.clear_and_type(formula, 'if(share_of_target >= 1, "met", 0)')
    s.click("//button[normalize-space()='Save step']")
    s.wait_text("//*[@role='alert']", "mixes text and a whole number")
    s.screenshot(f"{shots}/11-formula-error.png")
    s.clear_and_type(formula, 'if(share_of_target >= 1, "met", "below " & format(share_of_target * 100, 0) & "%")')
    s.click("//button[normalize-space()='Save step']")
    s.wait_text(STATUS, "need a run")
    s.click("//button[normalize-space()='Run analysis']")
    s.wait_text(STATUS, "All results current")
    s.wait_text(table, "below ")
    s.screenshot(f"{shots}/12-formula.png")
    s.click(node("Closest to target"))
    s.wait_text(node("Closest to target"), "2 rows")


if __name__ == "__main__":
    main()
