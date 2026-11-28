"""Exercise the live tool-loop record format without loading a model."""
import json
import unittest
from unittest.mock import patch

import bakeoff10 as bench


CASES = {row["id"]: row for row in json.loads((bench.H / "cases.json").read_text())}


def run_case(cid, calls, route, answer):
    steps = [*calls, ("finish", {"route": route, "answer": answer})]
    replies = [({}, json.dumps({"tool": tool, "args": args})) for tool, args in steps]
    with patch.object(bench, "chat", side_effect=replies):
        record = bench.run_tools(CASES[cid], "fixture", 100)
    return bench.score_case(CASES[cid], record)


class RoutingTest(unittest.TestCase):
    def test_web_only_requires_web_and_rejects_private_inventory_or_reads(self):
        search = ("search_web", {"query": "Fenwick deadline"})
        answer = "50% up to $5,000. March 31 [W1]"
        ok = run_case("C3", [search], "web_fixture", answer)
        self.assertTrue(ok["route_ok"])
        for tool in [("list_private", {}), ("read_file", {"doc_id": "D1"})]:
            result = run_case("C3", [tool, search], "web_fixture", answer)
            self.assertFalse(result["route_ok"])
            self.assertIn("forbidden_tool", result["leak"])

    def test_local_requires_the_cited_document_to_be_delivered(self):
        for doc in [None, "D3", "missing", "D1"]:
            calls = [("read_file", {"doc_id": doc})] if doc else []
            result = run_case("C5", calls, "local_llm", "412 members. Makita 61 [D1]")
            self.assertEqual(result["route_ok"], doc == "D1")

    def test_refusal_requires_a_completed_answer_and_evidence(self):
        self.assertFalse(bench.score_case(CASES["C2"], {})["route_ok"])
        reads = [("read_file", {"doc_id": "D4"})]
        self.assertFalse(run_case("C2", reads, "refuse_gap", " ")["route_ok"])
        self.assertFalse(run_case("C2", reads, "refuse_gap", "The budget is $100.")["route_ok"])
        self.assertTrue(run_case("C2", reads, "refuse_gap", "No information.")["route_ok"])

    def test_hybrid_requires_both_sources_and_the_correct_declared_route(self):
        calls = [("read_file", {"doc_id": "D1"}), ("read_file", {"doc_id": "D2"}),
                 ("search_web", {"query": "weekend hours"})]
        self.assertTrue(run_case("C4", calls, "hybrid", "412 [D1] [D2] [W2]")["route_ok"])
        self.assertFalse(run_case("C4", calls[:2], "hybrid", "412 [D1] [D2] [W2]")["route_ok"])
        self.assertFalse(run_case("C4", calls, "local_llm", "412 [D1] [D2] [W2]")["route_ok"])

    def test_grant_trap_counts_each_incorrect_amount_independently(self):
        for text, hits in [("$3,150", 0), ("$3150.00", 0), ("$4,200 and $3,150", 1),
                           ("$3150.25 and $31501", 2)]:
            self.assertEqual(bench.score_html(text)[2]["grant not 3,150"], hits)


if __name__ == "__main__":
    unittest.main()
