# Companies this radar can't reach

Hiring Radar crawls four things: Greenhouse boards, Lever boards, Workday
careers sites, and LinkedIn. Everything else is invisible to it.

These are the companies from the watchlist that use something else. They are
recorded here rather than left as silent gaps, because "no Google jobs on my
board" should be a fact you know about rather than one you slowly notice.

## Their own portal

No public API, and each would need its own scraper.

| Company | Where their jobs live |
|---|---|
| Google | `careers.google.com` |
| Amazon | `amazon.jobs` |
| Microsoft | `careers.microsoft.com` |
| LinkedIn | `linkedin.com/careers` (Microsoft-operated) |
| D. E. Shaw | `apply.deshaw.com` |
| Optum | `careers.unitedhealthgroup.com` |
| MakeMyTrip | `careers.makemytrip.com` |
| Goibibo | same site as MakeMyTrip — no separate board |
| Urban Company | `careers.urbancompany.com` (custom JS app) |
| INDmoney | `indmoney.com/careers` |
| Yatra | `tech.yatra.com/jobs` |
| Droom | `droom.in/career` |
| Times Internet | `timesinternet.in/careers` |
| Algorand | `algorand.co/algorand-foundation/careers` |
| KoineArth | no board at all — hires through LinkedIn, Cutshort, Instahyre |

## Another ATS

These have public-ish APIs. Each would be a new source module, and each one is
roughly the size of the Lever module — half a day, and then it works for every
company on that ATS rather than just one.

| ATS | Companies | Worth building? |
|---|---|---|
| **iCIMS** | ZS Associates, Publicis Sapient, Cvent | three from this list alone |
| **SmartRecruiters** | Zomato (`Zomato1`), Nagarro (`Nagarro1`) | public JSON API, easy |
| **Darwinbox** | Delhivery, Evalueserve | very common for Indian mid-caps |
| **Avature** | HSBC | also `hsbc.eightfold.ai` |
| **Taleo** | American Express | old, awkward, low priority |
| **SuccessFactors** | PayU | also on Trakstar for some India roles |
| **Trakstar** (ex-Recruiterbox) | OYO | one company, simple API |

SmartRecruiters is the obvious next one: a documented public API, and it covers
two companies here plus a long tail of Indian startups.

## Already being crawled

For the record, from the same watchlist:

- **Greenhouse** — Razorpay, Tower Research Capital, Graviton Research Capital,
  Arcesium
- **Lever** — CRED
- **Workday** — Sprinklr, Gojek (as GoTo Group), BlackRock, Barclays, Standard
  Chartered, AirAsia, Expedia, Adobe, Samsung R&D India

**Innovaccer** is a near miss: their Greenhouse board renders in a browser but
the public board API 404s for every spelling, so there is nothing to call. It
would need an HTML scrape rather than an API crawl.

## One trap worth remembering

`job-boards.greenhouse.io/pay2dc` turns up when searching for PayU's board. Its
jobs resolve to **PayPay India**, a different company. Adding it would quietly
fill the board with the wrong employer's listings — which is worse than an
empty board, because nothing about it looks wrong.
