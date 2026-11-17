//! Country names (#140, `mdm_geo`): each country of the IP table, in the interface's
//! language. Written by tools/geo/countries.py (names from lipis/flag-icons, MIT).

/// The name of the country with ISO 3166-1 code `code` (upper case), or None.
pub fn name(code: &str) -> Option<&'static str> {
    use crate::i18n::t;
    Some(match code {
        // i18n: a country's name (where a download's server is)
        "AD" => t("Andorra"),
        // i18n: a country's name (where a download's server is)
        "AE" => t("United Arab Emirates"),
        // i18n: a country's name (where a download's server is)
        "AF" => t("Afghanistan"),
        // i18n: a country's name (where a download's server is)
        "AG" => t("Antigua and Barbuda"),
        // i18n: a country's name (where a download's server is)
        "AI" => t("Anguilla"),
        // i18n: a country's name (where a download's server is)
        "AL" => t("Albania"),
        // i18n: a country's name (where a download's server is)
        "AM" => t("Armenia"),
        // i18n: a country's name (where a download's server is)
        "AO" => t("Angola"),
        // i18n: a country's name (where a download's server is)
        "AQ" => t("Antarctica"),
        // i18n: a country's name (where a download's server is)
        "AR" => t("Argentina"),
        // i18n: a country's name (where a download's server is)
        "AS" => t("American Samoa"),
        // i18n: a country's name (where a download's server is)
        "AT" => t("Austria"),
        // i18n: a country's name (where a download's server is)
        "AU" => t("Australia"),
        // i18n: a country's name (where a download's server is)
        "AW" => t("Aruba"),
        // i18n: a country's name (where a download's server is)
        "AX" => t("Aland Islands"),
        // i18n: a country's name (where a download's server is)
        "AZ" => t("Azerbaijan"),
        // i18n: a country's name (where a download's server is)
        "BA" => t("Bosnia and Herzegovina"),
        // i18n: a country's name (where a download's server is)
        "BB" => t("Barbados"),
        // i18n: a country's name (where a download's server is)
        "BD" => t("Bangladesh"),
        // i18n: a country's name (where a download's server is)
        "BE" => t("Belgium"),
        // i18n: a country's name (where a download's server is)
        "BF" => t("Burkina Faso"),
        // i18n: a country's name (where a download's server is)
        "BG" => t("Bulgaria"),
        // i18n: a country's name (where a download's server is)
        "BH" => t("Bahrain"),
        // i18n: a country's name (where a download's server is)
        "BI" => t("Burundi"),
        // i18n: a country's name (where a download's server is)
        "BJ" => t("Benin"),
        // i18n: a country's name (where a download's server is)
        "BL" => t("Saint Barthélemy"),
        // i18n: a country's name (where a download's server is)
        "BM" => t("Bermuda"),
        // i18n: a country's name (where a download's server is)
        "BN" => t("Brunei"),
        // i18n: a country's name (where a download's server is)
        "BO" => t("Bolivia"),
        // i18n: a country's name (where a download's server is)
        "BQ" => t("Bonaire, Sint Eustatius and Saba"),
        // i18n: a country's name (where a download's server is)
        "BR" => t("Brazil"),
        // i18n: a country's name (where a download's server is)
        "BS" => t("Bahamas"),
        // i18n: a country's name (where a download's server is)
        "BT" => t("Bhutan"),
        // i18n: a country's name (where a download's server is)
        "BV" => t("Bouvet Island"),
        // i18n: a country's name (where a download's server is)
        "BW" => t("Botswana"),
        // i18n: a country's name (where a download's server is)
        "BY" => t("Belarus"),
        // i18n: a country's name (where a download's server is)
        "BZ" => t("Belize"),
        // i18n: a country's name (where a download's server is)
        "CA" => t("Canada"),
        // i18n: a country's name (where a download's server is)
        "CC" => t("Cocos (Keeling) Islands"),
        // i18n: a country's name (where a download's server is)
        "CD" => t("DR Congo"),
        // i18n: a country's name (where a download's server is)
        "CF" => t("Central African Republic"),
        // i18n: a country's name (where a download's server is)
        "CG" => t("Congo"),
        // i18n: a country's name (where a download's server is)
        "CH" => t("Switzerland"),
        // i18n: a country's name (where a download's server is)
        "CI" => t("Côte d'Ivoire"),
        // i18n: a country's name (where a download's server is)
        "CK" => t("Cook Islands"),
        // i18n: a country's name (where a download's server is)
        "CL" => t("Chile"),
        // i18n: a country's name (where a download's server is)
        "CM" => t("Cameroon"),
        // i18n: a country's name (where a download's server is)
        "CN" => t("China"),
        // i18n: a country's name (where a download's server is)
        "CO" => t("Colombia"),
        // i18n: a country's name (where a download's server is)
        "CR" => t("Costa Rica"),
        // i18n: a country's name (where a download's server is)
        "CU" => t("Cuba"),
        // i18n: a country's name (where a download's server is)
        "CV" => t("Cabo Verde"),
        // i18n: a country's name (where a download's server is)
        "CW" => t("Curaçao"),
        // i18n: a country's name (where a download's server is)
        "CX" => t("Christmas Island"),
        // i18n: a country's name (where a download's server is)
        "CY" => t("Cyprus"),
        // i18n: a country's name (where a download's server is)
        "CZ" => t("Czech Republic"),
        // i18n: a country's name (where a download's server is)
        "DE" => t("Germany"),
        // i18n: a country's name (where a download's server is)
        "DJ" => t("Djibouti"),
        // i18n: a country's name (where a download's server is)
        "DK" => t("Denmark"),
        // i18n: a country's name (where a download's server is)
        "DM" => t("Dominica"),
        // i18n: a country's name (where a download's server is)
        "DO" => t("Dominican Republic"),
        // i18n: a country's name (where a download's server is)
        "DZ" => t("Algeria"),
        // i18n: a country's name (where a download's server is)
        "EC" => t("Ecuador"),
        // i18n: a country's name (where a download's server is)
        "EE" => t("Estonia"),
        // i18n: a country's name (where a download's server is)
        "EG" => t("Egypt"),
        // i18n: a country's name (where a download's server is)
        "EH" => t("Western Sahara"),
        // i18n: a country's name (where a download's server is)
        "ER" => t("Eritrea"),
        // i18n: a country's name (where a download's server is)
        "ES" => t("Spain"),
        // i18n: a country's name (where a download's server is)
        "ET" => t("Ethiopia"),
        // i18n: a country's name (where a download's server is)
        "FI" => t("Finland"),
        // i18n: a country's name (where a download's server is)
        "FJ" => t("Fiji"),
        // i18n: a country's name (where a download's server is)
        "FK" => t("Falkland Islands"),
        // i18n: a country's name (where a download's server is)
        "FM" => t("Micronesia"),
        // i18n: a country's name (where a download's server is)
        "FO" => t("Faroe Islands"),
        // i18n: a country's name (where a download's server is)
        "FR" => t("France"),
        // i18n: a country's name (where a download's server is)
        "GA" => t("Gabon"),
        // i18n: a country's name (where a download's server is)
        "GB" => t("United Kingdom"),
        // i18n: a country's name (where a download's server is)
        "GD" => t("Grenada"),
        // i18n: a country's name (where a download's server is)
        "GE" => t("Georgia"),
        // i18n: a country's name (where a download's server is)
        "GF" => t("French Guiana"),
        // i18n: a country's name (where a download's server is)
        "GG" => t("Guernsey"),
        // i18n: a country's name (where a download's server is)
        "GH" => t("Ghana"),
        // i18n: a country's name (where a download's server is)
        "GI" => t("Gibraltar"),
        // i18n: a country's name (where a download's server is)
        "GL" => t("Greenland"),
        // i18n: a country's name (where a download's server is)
        "GM" => t("Gambia"),
        // i18n: a country's name (where a download's server is)
        "GN" => t("Guinea"),
        // i18n: a country's name (where a download's server is)
        "GP" => t("Guadeloupe"),
        // i18n: a country's name (where a download's server is)
        "GQ" => t("Equatorial Guinea"),
        // i18n: a country's name (where a download's server is)
        "GR" => t("Greece"),
        // i18n: a country's name (where a download's server is)
        "GS" => t("South Georgia and the South Sandwich Islands"),
        // i18n: a country's name (where a download's server is)
        "GT" => t("Guatemala"),
        // i18n: a country's name (where a download's server is)
        "GU" => t("Guam"),
        // i18n: a country's name (where a download's server is)
        "GW" => t("Guinea-Bissau"),
        // i18n: a country's name (where a download's server is)
        "GY" => t("Guyana"),
        // i18n: a country's name (where a download's server is)
        "HK" => t("Hong Kong"),
        // i18n: a country's name (where a download's server is)
        "HM" => t("Heard Island and McDonald Islands"),
        // i18n: a country's name (where a download's server is)
        "HN" => t("Honduras"),
        // i18n: a country's name (where a download's server is)
        "HR" => t("Croatia"),
        // i18n: a country's name (where a download's server is)
        "HT" => t("Haiti"),
        // i18n: a country's name (where a download's server is)
        "HU" => t("Hungary"),
        // i18n: a country's name (where a download's server is)
        "ID" => t("Indonesia"),
        // i18n: a country's name (where a download's server is)
        "IE" => t("Ireland"),
        // i18n: a country's name (where a download's server is)
        "IL" => t("Israel"),
        // i18n: a country's name (where a download's server is)
        "IM" => t("Isle of Man"),
        // i18n: a country's name (where a download's server is)
        "IN" => t("India"),
        // i18n: a country's name (where a download's server is)
        "IO" => t("British Indian Ocean Territory"),
        // i18n: a country's name (where a download's server is)
        "IQ" => t("Iraq"),
        // i18n: a country's name (where a download's server is)
        "IR" => t("Iran"),
        // i18n: a country's name (where a download's server is)
        "IS" => t("Iceland"),
        // i18n: a country's name (where a download's server is)
        "IT" => t("Italy"),
        // i18n: a country's name (where a download's server is)
        "JE" => t("Jersey"),
        // i18n: a country's name (where a download's server is)
        "JM" => t("Jamaica"),
        // i18n: a country's name (where a download's server is)
        "JO" => t("Jordan"),
        // i18n: a country's name (where a download's server is)
        "JP" => t("Japan"),
        // i18n: a country's name (where a download's server is)
        "KE" => t("Kenya"),
        // i18n: a country's name (where a download's server is)
        "KG" => t("Kyrgyzstan"),
        // i18n: a country's name (where a download's server is)
        "KH" => t("Cambodia"),
        // i18n: a country's name (where a download's server is)
        "KI" => t("Kiribati"),
        // i18n: a country's name (where a download's server is)
        "KM" => t("Comoros"),
        // i18n: a country's name (where a download's server is)
        "KN" => t("Saint Kitts and Nevis"),
        // i18n: a country's name (where a download's server is)
        "KP" => t("North Korea"),
        // i18n: a country's name (where a download's server is)
        "KR" => t("South Korea"),
        // i18n: a country's name (where a download's server is)
        "KW" => t("Kuwait"),
        // i18n: a country's name (where a download's server is)
        "KY" => t("Cayman Islands"),
        // i18n: a country's name (where a download's server is)
        "KZ" => t("Kazakhstan"),
        // i18n: a country's name (where a download's server is)
        "LA" => t("Laos"),
        // i18n: a country's name (where a download's server is)
        "LB" => t("Lebanon"),
        // i18n: a country's name (where a download's server is)
        "LC" => t("Saint Lucia"),
        // i18n: a country's name (where a download's server is)
        "LI" => t("Liechtenstein"),
        // i18n: a country's name (where a download's server is)
        "LK" => t("Sri Lanka"),
        // i18n: a country's name (where a download's server is)
        "LR" => t("Liberia"),
        // i18n: a country's name (where a download's server is)
        "LS" => t("Lesotho"),
        // i18n: a country's name (where a download's server is)
        "LT" => t("Lithuania"),
        // i18n: a country's name (where a download's server is)
        "LU" => t("Luxembourg"),
        // i18n: a country's name (where a download's server is)
        "LV" => t("Latvia"),
        // i18n: a country's name (where a download's server is)
        "LY" => t("Libya"),
        // i18n: a country's name (where a download's server is)
        "MA" => t("Morocco"),
        // i18n: a country's name (where a download's server is)
        "MC" => t("Monaco"),
        // i18n: a country's name (where a download's server is)
        "MD" => t("Moldova"),
        // i18n: a country's name (where a download's server is)
        "ME" => t("Montenegro"),
        // i18n: a country's name (where a download's server is)
        "MF" => t("Saint Martin"),
        // i18n: a country's name (where a download's server is)
        "MG" => t("Madagascar"),
        // i18n: a country's name (where a download's server is)
        "MH" => t("Marshall Islands"),
        // i18n: a country's name (where a download's server is)
        "MK" => t("North Macedonia"),
        // i18n: a country's name (where a download's server is)
        "ML" => t("Mali"),
        // i18n: a country's name (where a download's server is)
        "MM" => t("Myanmar"),
        // i18n: a country's name (where a download's server is)
        "MN" => t("Mongolia"),
        // i18n: a country's name (where a download's server is)
        "MO" => t("Macau"),
        // i18n: a country's name (where a download's server is)
        "MP" => t("Northern Mariana Islands"),
        // i18n: a country's name (where a download's server is)
        "MQ" => t("Martinique"),
        // i18n: a country's name (where a download's server is)
        "MR" => t("Mauritania"),
        // i18n: a country's name (where a download's server is)
        "MS" => t("Montserrat"),
        // i18n: a country's name (where a download's server is)
        "MT" => t("Malta"),
        // i18n: a country's name (where a download's server is)
        "MU" => t("Mauritius"),
        // i18n: a country's name (where a download's server is)
        "MV" => t("Maldives"),
        // i18n: a country's name (where a download's server is)
        "MW" => t("Malawi"),
        // i18n: a country's name (where a download's server is)
        "MX" => t("Mexico"),
        // i18n: a country's name (where a download's server is)
        "MY" => t("Malaysia"),
        // i18n: a country's name (where a download's server is)
        "MZ" => t("Mozambique"),
        // i18n: a country's name (where a download's server is)
        "NA" => t("Namibia"),
        // i18n: a country's name (where a download's server is)
        "NC" => t("New Caledonia"),
        // i18n: a country's name (where a download's server is)
        "NE" => t("Niger"),
        // i18n: a country's name (where a download's server is)
        "NF" => t("Norfolk Island"),
        // i18n: a country's name (where a download's server is)
        "NG" => t("Nigeria"),
        // i18n: a country's name (where a download's server is)
        "NI" => t("Nicaragua"),
        // i18n: a country's name (where a download's server is)
        "NL" => t("Netherlands"),
        // i18n: a country's name (where a download's server is)
        "NO" => t("Norway"),
        // i18n: a country's name (where a download's server is)
        "NP" => t("Nepal"),
        // i18n: a country's name (where a download's server is)
        "NR" => t("Nauru"),
        // i18n: a country's name (where a download's server is)
        "NU" => t("Niue"),
        // i18n: a country's name (where a download's server is)
        "NZ" => t("New Zealand"),
        // i18n: a country's name (where a download's server is)
        "OM" => t("Oman"),
        // i18n: a country's name (where a download's server is)
        "PA" => t("Panama"),
        // i18n: a country's name (where a download's server is)
        "PE" => t("Peru"),
        // i18n: a country's name (where a download's server is)
        "PF" => t("French Polynesia"),
        // i18n: a country's name (where a download's server is)
        "PG" => t("Papua New Guinea"),
        // i18n: a country's name (where a download's server is)
        "PH" => t("Philippines"),
        // i18n: a country's name (where a download's server is)
        "PK" => t("Pakistan"),
        // i18n: a country's name (where a download's server is)
        "PL" => t("Poland"),
        // i18n: a country's name (where a download's server is)
        "PM" => t("Saint Pierre and Miquelon"),
        // i18n: a country's name (where a download's server is)
        "PN" => t("Pitcairn"),
        // i18n: a country's name (where a download's server is)
        "PR" => t("Puerto Rico"),
        // i18n: a country's name (where a download's server is)
        "PS" => t("Palestine"),
        // i18n: a country's name (where a download's server is)
        "PT" => t("Portugal"),
        // i18n: a country's name (where a download's server is)
        "PW" => t("Palau"),
        // i18n: a country's name (where a download's server is)
        "PY" => t("Paraguay"),
        // i18n: a country's name (where a download's server is)
        "QA" => t("Qatar"),
        // i18n: a country's name (where a download's server is)
        "RE" => t("Réunion"),
        // i18n: a country's name (where a download's server is)
        "RO" => t("Romania"),
        // i18n: a country's name (where a download's server is)
        "RS" => t("Serbia"),
        // i18n: a country's name (where a download's server is)
        "RU" => t("Russia"),
        // i18n: a country's name (where a download's server is)
        "RW" => t("Rwanda"),
        // i18n: a country's name (where a download's server is)
        "SA" => t("Saudi Arabia"),
        // i18n: a country's name (where a download's server is)
        "SB" => t("Solomon Islands"),
        // i18n: a country's name (where a download's server is)
        "SC" => t("Seychelles"),
        // i18n: a country's name (where a download's server is)
        "SD" => t("Sudan"),
        // i18n: a country's name (where a download's server is)
        "SE" => t("Sweden"),
        // i18n: a country's name (where a download's server is)
        "SG" => t("Singapore"),
        // i18n: a country's name (where a download's server is)
        "SH" => t("Saint Helena, Ascension and Tristan da Cunha"),
        // i18n: a country's name (where a download's server is)
        "SI" => t("Slovenia"),
        // i18n: a country's name (where a download's server is)
        "SJ" => t("Svalbard and Jan Mayen"),
        // i18n: a country's name (where a download's server is)
        "SK" => t("Slovakia"),
        // i18n: a country's name (where a download's server is)
        "SL" => t("Sierra Leone"),
        // i18n: a country's name (where a download's server is)
        "SM" => t("San Marino"),
        // i18n: a country's name (where a download's server is)
        "SN" => t("Senegal"),
        // i18n: a country's name (where a download's server is)
        "SO" => t("Somalia"),
        // i18n: a country's name (where a download's server is)
        "SR" => t("Suriname"),
        // i18n: a country's name (where a download's server is)
        "SS" => t("South Sudan"),
        // i18n: a country's name (where a download's server is)
        "ST" => t("Sao Tome and Principe"),
        // i18n: a country's name (where a download's server is)
        "SV" => t("El Salvador"),
        // i18n: a country's name (where a download's server is)
        "SX" => t("Sint Maarten"),
        // i18n: a country's name (where a download's server is)
        "SY" => t("Syria"),
        // i18n: a country's name (where a download's server is)
        "SZ" => t("Eswatini"),
        // i18n: a country's name (where a download's server is)
        "TC" => t("Turks and Caicos Islands"),
        // i18n: a country's name (where a download's server is)
        "TD" => t("Chad"),
        // i18n: a country's name (where a download's server is)
        "TF" => t("French Southern Territories"),
        // i18n: a country's name (where a download's server is)
        "TG" => t("Togo"),
        // i18n: a country's name (where a download's server is)
        "TH" => t("Thailand"),
        // i18n: a country's name (where a download's server is)
        "TJ" => t("Tajikistan"),
        // i18n: a country's name (where a download's server is)
        "TK" => t("Tokelau"),
        // i18n: a country's name (where a download's server is)
        "TL" => t("Timor-Leste"),
        // i18n: a country's name (where a download's server is)
        "TM" => t("Turkmenistan"),
        // i18n: a country's name (where a download's server is)
        "TN" => t("Tunisia"),
        // i18n: a country's name (where a download's server is)
        "TO" => t("Tonga"),
        // i18n: a country's name (where a download's server is)
        "TR" => t("Türkiye"),
        // i18n: a country's name (where a download's server is)
        "TT" => t("Trinidad and Tobago"),
        // i18n: a country's name (where a download's server is)
        "TV" => t("Tuvalu"),
        // i18n: a country's name (where a download's server is)
        "TW" => t("Taiwan"),
        // i18n: a country's name (where a download's server is)
        "TZ" => t("Tanzania"),
        // i18n: a country's name (where a download's server is)
        "UA" => t("Ukraine"),
        // i18n: a country's name (where a download's server is)
        "UG" => t("Uganda"),
        // i18n: a country's name (where a download's server is)
        "UM" => t("United States Minor Outlying Islands"),
        // i18n: a country's name (where a download's server is)
        "US" => t("United States"),
        // i18n: a country's name (where a download's server is)
        "UY" => t("Uruguay"),
        // i18n: a country's name (where a download's server is)
        "UZ" => t("Uzbekistan"),
        // i18n: a country's name (where a download's server is)
        "VA" => t("Vatican City"),
        // i18n: a country's name (where a download's server is)
        "VC" => t("Saint Vincent and the Grenadines"),
        // i18n: a country's name (where a download's server is)
        "VE" => t("Venezuela"),
        // i18n: a country's name (where a download's server is)
        "VG" => t("Virgin Islands (British)"),
        // i18n: a country's name (where a download's server is)
        "VI" => t("Virgin Islands (U.S.)"),
        // i18n: a country's name (where a download's server is)
        "VN" => t("Vietnam"),
        // i18n: a country's name (where a download's server is)
        "VU" => t("Vanuatu"),
        // i18n: a country's name (where a download's server is)
        "WF" => t("Wallis and Futuna"),
        // i18n: a country's name (where a download's server is)
        "WS" => t("Samoa"),
        // i18n: a country's name (where a download's server is)
        "XK" => t("Kosovo"),
        // i18n: a country's name (where a download's server is)
        "YE" => t("Yemen"),
        // i18n: a country's name (where a download's server is)
        "YT" => t("Mayotte"),
        // i18n: a country's name (where a download's server is)
        "ZA" => t("South Africa"),
        // i18n: a country's name (where a download's server is)
        "ZM" => t("Zambia"),
        // i18n: a country's name (where a download's server is)
        "ZW" => t("Zimbabwe"),
        _ => return None,
    })
}
