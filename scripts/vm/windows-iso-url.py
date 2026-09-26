import json, re, time, uuid, urllib.request, urllib.parse, http.cookiejar, sys
ORG, PROFILE, INSTANCE = "y6jn8c31", "606624d44113", "560dc9f3-1aa5-4a2f-b63c-9e18f8d0e175"
EDITION = sys.argv[1] if len(sys.argv) > 1 else "3324"  # Win11 ARM64
LANG_WANT = sys.argv[2] if len(sys.argv) > 2 else "Chinese Simplified"
cj = http.cookiejar.CookieJar()
op = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(cj))
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36"
def get(url, ref=None):
    req = urllib.request.Request(url, headers={"User-Agent": UA, **({"Referer": ref} if ref else {})})
    with op.open(req, timeout=30) as r:
        return r.read().decode("utf-8", "replace")
sid = str(uuid.uuid4())
get(f"https://vlscppe.microsoft.com/tags?org_id={ORG}&session_id={sid}")
js = get(f"https://ov-df.microsoft.com/mdt.js?instanceId={INSTANCE}&PageId=si&session_id={sid}")
w = re.search(r"[?&]w=([A-F0-9]+)", js).group(1)
rt = re.search(r'rticks\="\+?(\d+)', js).group(1)
get(f"https://ov-df.microsoft.com/?session_id={sid}&CustomerId={INSTANCE}&PageId=si&w={w}&mdt={int(time.time()*1000)}&rticks={rt}")
q = f"https://www.microsoft.com/software-download-connector/api/getskuinformationbyproductedition?profile={PROFILE}&productEditionId={EDITION}&SKU=undefined&friendlyFileName=undefined&Locale=en-US&sessionID={sid}"
for attempt in range(4):
    skus = json.loads(get(q))
    if skus.get("Skus"): break
    time.sleep(2)
langs = {s["Language"]: s["Id"] for s in skus["Skus"]}
print("languages:", ", ".join(sorted(langs)), file=sys.stderr)
sku = langs[LANG_WANT]
q = f"https://www.microsoft.com/software-download-connector/api/GetProductDownloadLinksBySku?profile={PROFILE}&productEditionId=undefined&SKU={sku}&friendlyFileName=undefined&Locale=en-US&sessionID={sid}"
r = json.loads(get(q, ref="https://www.microsoft.com/software-download/windows11"))
if r.get("Errors"):
    print("error:", r["Errors"], file=sys.stderr); sys.exit(1)
for o in r["ProductDownloadOptions"]:
    print(o["DownloadType"], o["Uri"])
