import json, sys
orders = json.load(open(sys.argv[1])) if len(sys.argv) > 1 else []
print(f"{len(orders)} orders today")
