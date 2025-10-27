# Country Currency & Exchange API

This is a RESTful API built in Rust using Actix-web, fetching country data and exchange rates, storing in MySQL, and providing CRUD operations with caching.

## Setup Instructions

1. **Install Rust**: Ensure Rust is installed via [rustup](https://rustup.rs/).

2. **Clone and Build**:
git clone <repo>  # Or create new project and replace files
cd country-currency-api
cargo build


3. **Database Setup**:
- Install MySQL and create a database (e.g., `countries_db`).
- Run the following SQL to create the table:
  ```sql
  CREATE DATABASE IF NOT EXISTS countries_db;
  USE countries_db;

  CREATE TABLE countries (
      id INT AUTO_INCREMENT PRIMARY KEY,
      name VARCHAR(255) NOT NULL,
      capital VARCHAR(255) NULL,
      region VARCHAR(255) NULL,
      population BIGINT NOT NULL,
      currency_code VARCHAR(3) NULL,
      exchange_rate DOUBLE NULL,
      estimated_gdp DOUBLE NOT NULL DEFAULT 0.0,
      flag_url TEXT NULL,
      last_refreshed_at DATETIME NOT NULL
  );

- Note: No unique constraint on name to allow case-insensitive handling in code.


4. Environment Configuration:
- Create a .env file in the project root:
DATABASE_URL=mysql://username:password@localhost/countries_db
PORT=8080
- Replace username, password, localhost, and countries_db with your MySQL details.

3. Run the Server:

cargo run

- The server will start on http://localhost:8080 (or specified PORT).


2. API Usage:

- Refresh Data: POST /countries/refresh (triggers fetch and cache update).
- Get All Countries: GET /countries?region=Africa&currency=NGN&sort=gdp_desc
- Get Country: GET /countries/Nigeria
- Delete Country: DELETE /countries/Nigeria
- Status: GET /status
- Summary Image: GET /countries/image (serves PNG; generates on refresh).


1. Notes:

- Data is only updated on /countries/refresh.
- Estimated GDP uses formula: population * random(1000.0..2000.0) / exchange_rate (in USD approximation; handles null rates as 0).
- Case-insensitive matching for name-based operations.
- Image generated using Plotters crate; requires no external fonts.
- Error handling follows specified JSON formats.
- For production, consider adding migrations with sqlx-cli: cargo install sqlx-cli, then sqlx migrate add init etc.
- Dependencies are up-to-date as of October 2025; run cargo update if needed.



### Edge Cases Handled

- Countries without currencies: currency_code=null, exchange_rate=null, estimated_gdp=0.
- Unknown currencies: Same as above.
- API failures/timeouts: 503 with details, no DB changes.
- Missing required fields in API data: Skip country.
- Empty/multiple currencies: Use first non-empty code.
- No sort/filter: Returns all unsorted.
- Invalid query params: Ignored if empty.
- No image: 404 JSON error.
- Empty DB: Status shows 0 and null timestamp.
- Case variations in name: Handled via LOWER() in queries.

### Testing
Use tools like curl or Postman. Example:

- Refresh: curl -X POST http://localhost:8080/countries/refresh
- Get Africa: curl "http://localhost:8080/countries?region=Africa"

